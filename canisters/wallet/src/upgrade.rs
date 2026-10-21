//! Self-upgrade, as "Self-upgrade" in `docs/design.md` describes it. An
//! `Upgrade` action stages a signed release's chunks in the wallet's own chunk
//! store, then installs them with a one-way `install_chunked_code` to the
//! wallet itself. Pure: the caller passes the time and stores the result.
//!
//! The wallet has one chunk store, so one execution stages at a time. While
//! the install is on its way the wallet starts no call: a reply that reached
//! the new version would run a callback whose state went with the old heap.
//! The new version's `post_upgrade` ends the upgrade `Idle`. An old version
//! still running at the deadline means the install failed, and the upgrade
//! ends `Failed`.

use crate::policy::HOUR_NS;
use crate::transfers::STALE_CALL_NS;
use crate::{ActionId, UpgradeState, WalletError};
use candid::CandidType;
use release::{ReleaseManifest, Signers};
use serde::Deserialize;
use serde_bytes::ByteBuf;

/// How long the old version waits for the new one before it calls the
/// upgrade failed. An install runs within a few rounds; a release whose
/// `post_upgrade` traps leaves the old version running, which sees this
/// deadline pass.
pub const INSTALL_DEADLINE_NS: u64 = HOUR_NS;

/// The upgrade as stored: what the interface shows, and what staging and
/// installing need.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StoredUpgrade {
    pub state: UpgradeState,
    /// The `Upgrade` action whose release is staging or installing.
    pub action: Option<ActionId>,
    /// That release.
    pub manifest: Option<ReleaseManifest>,
    /// The executions started so far; the latest owns the staging.
    pub executions: u64,
    /// When the execution in flight last went on after a call; `None` once
    /// it returned.
    pub in_flight_since_ns: Option<u64>,
    /// Whether the chunk store holds chunks that no staging needs any more.
    pub clear_store: bool,
}

impl Default for StoredUpgrade {
    fn default() -> Self {
        StoredUpgrade {
            state: UpgradeState::Idle,
            action: None,
            manifest: None,
            executions: 0,
            in_flight_since_ns: None,
            clear_store: false,
        }
    }
}

/// Checks a release against the wallet: its form, the signatures of the
/// release signer keys the wallet pins, and that it was tested to upgrade
/// from the module the wallet runs, `module_hash`.
pub fn check_release(
    manifest: &ReleaseManifest,
    signers: &Signers,
    module_hash: Option<&[u8]>,
) -> Result<(), String> {
    let version = &manifest.version;
    manifest
        .check_form()
        .map_err(|reason| format!("release {version} is out of form: {reason}"))?;
    signers.check(manifest).map_err(|missing| {
        format!(
            "release {version} has {} of the {} signatures by this wallet's release signers \
             it needs",
            missing.have, missing.need
        )
    })?;
    let tested = module_hash.is_some_and(|hash| {
        manifest
            .tested_from
            .iter()
            .any(|from| from.as_slice() == hash)
    });
    if !tested {
        return Err(format!(
            "release {version} was not tested to upgrade from the version this wallet runs"
        ));
    }
    Ok(())
}

/// Checks the release an `Upgrade` action names before it is proposed.
pub fn check_action(version: &str, wasm_sha256: &[u8]) -> Result<(), WalletError> {
    release::check_version(version).map_err(WalletError::InvalidArgument)?;
    if wasm_sha256.len() != 32 {
        return Err(WalletError::InvalidArgument(
            "wasm_sha256 must be 32 bytes".to_string(),
        ));
    }
    Ok(())
}

impl StoredUpgrade {
    pub fn is_installing(&self) -> bool {
        matches!(self.state, UpgradeState::Installing { .. })
    }

    /// The upgrade as the interface shows it at `now`: an install past its
    /// deadline has failed, whether or not that was written down yet.
    pub fn view(&self, now: u64) -> UpgradeState {
        let mut current = self.clone();
        current.check_deadline(now);
        current.state
    }

    /// Starts an execution and returns its number. Refuses while an install
    /// is on its way, and while another execution is in flight. One still
    /// marked in flight after `STALE_CALL_NS` never returned to the wallet's
    /// code, since each of its calls answers within 300 seconds.
    pub fn begin(&mut self, now: u64) -> Result<u64, WalletError> {
        if self.is_installing() {
            return Err(WalletError::UpgradeInProgress);
        }
        if let Some(since) = self.in_flight_since_ns
            && now < since.saturating_add(STALE_CALL_NS)
        {
            return Err(WalletError::UpgradeInProgress);
        }
        self.executions += 1;
        self.in_flight_since_ns = Some(now);
        Ok(self.executions)
    }

    /// Whether execution `execution` is the one in flight.
    pub fn owns(&self, execution: u64) -> bool {
        self.executions == execution && self.in_flight_since_ns.is_some()
    }

    /// Goes on with execution `execution` after a call, at `now`.
    pub fn resume(&mut self, execution: u64, now: u64) -> Result<(), WalletError> {
        if !self.owns(execution) {
            return Err(WalletError::InvalidArgument(
                "another execution of an upgrade took over".to_string(),
            ));
        }
        self.in_flight_since_ns = Some(now);
        Ok(())
    }

    /// Records that the execution in flight returned, or will not go on.
    pub fn end(&mut self, execution: u64) {
        if self.owns(execution) {
            self.in_flight_since_ns = None;
        }
    }

    /// Stages `manifest` for action `id` and returns the first chunk still to
    /// upload. The chunks of the same release that an earlier execution
    /// uploaded stay; any other chunks in the store are cleared first, which
    /// `clear_store` asks for.
    pub fn stage(&mut self, id: ActionId, manifest: ReleaseManifest) -> u32 {
        let total = u32::try_from(manifest.chunk_hashes.len()).unwrap_or(u32::MAX);
        if let UpgradeState::Staging {
            chunks_uploaded, ..
        } = self.state
            && self.manifest.as_ref() == Some(&manifest)
        {
            self.action = Some(id);
            return chunks_uploaded;
        }
        self.state = UpgradeState::Staging {
            version: manifest.version.clone(),
            chunks_uploaded: 0,
            chunks_total: total,
        };
        self.action = Some(id);
        self.manifest = Some(manifest);
        self.clear_store = true;
        0
    }

    /// Records that the chunk store holds chunk `index` of the staged
    /// release.
    pub fn uploaded(&mut self, index: u32) {
        if let UpgradeState::Staging {
            chunks_uploaded, ..
        } = &mut self.state
        {
            *chunks_uploaded = index + 1;
        }
    }

    /// Whether every chunk of the staged release is in the chunk store.
    pub fn is_staged(&self) -> bool {
        matches!(
            self.state,
            UpgradeState::Staging { chunks_uploaded, chunks_total, .. }
                if chunks_uploaded == chunks_total
        )
    }

    /// Records that the staged release's install was sent at `now`.
    pub fn installing(&mut self, now: u64) {
        let Some(manifest) = &self.manifest else {
            return;
        };
        self.state = UpgradeState::Installing {
            version: manifest.version.clone(),
            wasm_sha256: manifest.wasm_sha256.clone(),
            started_at_ns: now,
            deadline_ns: now.saturating_add(INSTALL_DEADLINE_NS),
        };
        self.in_flight_since_ns = None;
    }

    /// Ends an install that ran: the new version calls it from
    /// `post_upgrade`. Returns the action and the version installed.
    pub fn installed(&mut self) -> Option<(Option<ActionId>, String)> {
        let UpgradeState::Installing { version, .. } = &self.state else {
            return None;
        };
        let version = version.clone();
        self.state = UpgradeState::Idle;
        self.manifest = None;
        self.clear_store = true;
        Some((self.action.take(), version))
    }

    /// Ends an install the old version outlived past its deadline: the new
    /// version's `post_upgrade` trapped, or the install was refused. Returns
    /// the action and the version that failed.
    pub fn check_deadline(&mut self, now: u64) -> Option<(Option<ActionId>, String)> {
        let UpgradeState::Installing {
            version,
            deadline_ns,
            ..
        } = &self.state
        else {
            return None;
        };
        if now < *deadline_ns {
            return None;
        }
        let version = version.clone();
        self.state = UpgradeState::Failed {
            version: version.clone(),
            reason: failure(&version),
            at_ns: now,
        };
        self.manifest = None;
        self.clear_store = true;
        Some((self.action.take(), version))
    }

    /// Drops the staging of action `id`, once it was rejected. An install on
    /// its way is not dropped.
    pub fn drop_staging(&mut self, id: ActionId) -> bool {
        if self.action != Some(id) || !matches!(self.state, UpgradeState::Staging { .. }) {
            return false;
        }
        self.state = UpgradeState::Idle;
        self.action = None;
        self.manifest = None;
        self.clear_store = true;
        true
    }

    /// Whether the timer may clear the chunk store now: it holds chunks no
    /// staging or install needs.
    pub fn may_clear_store(&self) -> bool {
        self.clear_store && matches!(self.state, UpgradeState::Idle | UpgradeState::Failed { .. })
    }

    /// When the timer must run next: at the install's deadline, or at `now`
    /// while the chunk store may be cleared.
    pub fn wake_at(&self, now: u64) -> Option<u64> {
        match self.state {
            UpgradeState::Installing { deadline_ns, .. } => Some(deadline_ns),
            _ => self.may_clear_store().then_some(now),
        }
    }

    /// The chunk hashes and module hash `install_chunked_code` takes.
    pub fn install_hashes(&self) -> Option<(Vec<ByteBuf>, ByteBuf)> {
        let manifest = self.manifest.as_ref()?;
        Some((manifest.chunk_hashes.clone(), manifest.wasm_sha256.clone()))
    }
}

/// Why an install the old version outlived failed.
pub fn failure(version: &str) -> String {
    format!(
        "release {version} did not finish installing: the version before it still runs, so \
         the new version's post_upgrade trapped or the install was refused"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    const T0: u64 = 1_000 * HOUR_NS;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn signers() -> Signers {
        let keys: Vec<[u8; 32]> = (1..=3)
            .map(|seed| key(seed).verifying_key().to_bytes())
            .collect();
        Signers::new(&keys, 2).unwrap()
    }

    const RUNNING: [u8; 32] = [5; 32];

    fn manifest(version: &str, wasm: &[u8]) -> ReleaseManifest {
        let mut manifest = ReleaseManifest::new(
            version.to_string(),
            wasm,
            vec![RUNNING],
            format!("https://example.org/{version}"),
            T0,
        );
        manifest.sign(&key(1));
        manifest.sign(&key(2));
        manifest
    }

    fn wasm() -> Vec<u8> {
        (0..release::CHUNK_SIZE + 10).map(|i| i as u8).collect()
    }

    #[test]
    fn a_release_needs_the_pinned_signers_and_a_tested_path() {
        let release = manifest("1.0.0", &wasm());
        assert_eq!(check_release(&release, &signers(), Some(&RUNNING)), Ok(()));

        let untested = check_release(&release, &signers(), Some(&[6; 32])).unwrap_err();
        assert!(untested.contains("not tested to upgrade"), "{untested}");
        assert!(check_release(&release, &signers(), None).is_err());

        let mut one = release.clone();
        one.signatures.truncate(1);
        let reason = check_release(&one, &signers(), Some(&RUNNING)).unwrap_err();
        assert!(reason.contains("1 of the 2 signatures"), "{reason}");

        let mut stranger = manifest("1.0.0", &wasm());
        stranger.signatures.clear();
        stranger.sign(&key(1));
        stranger.sign(&key(9));
        assert!(check_release(&stranger, &signers(), Some(&RUNNING)).is_err());

        let mut out_of_form = release.clone();
        out_of_form.chunk_hashes.pop();
        let reason = check_release(&out_of_form, &signers(), Some(&RUNNING)).unwrap_err();
        assert!(reason.contains("out of form"), "{reason}");
    }

    #[test]
    fn an_action_names_a_version_and_a_hash() {
        assert_eq!(check_action("1.0.0", &[0; 32]), Ok(()));
        assert!(check_action("", &[0; 32]).is_err());
        assert!(check_action("1.0 0", &[0; 32]).is_err());
        assert!(check_action("1.0.0", &[0; 31]).is_err());
    }

    #[test]
    fn one_execution_stages_at_a_time_and_a_stale_one_is_taken_over() {
        let mut upgrade = StoredUpgrade::default();
        let first = upgrade.begin(T0).unwrap();
        assert_eq!(upgrade.begin(T0 + 1), Err(WalletError::UpgradeInProgress));
        upgrade.resume(first, T0 + 2).unwrap();
        let second = upgrade.begin(T0 + 2 + STALE_CALL_NS).unwrap();
        assert!(!upgrade.owns(first));
        assert!(upgrade.resume(first, T0 + 3 + STALE_CALL_NS).is_err());
        upgrade.end(first);
        assert!(upgrade.owns(second));
        upgrade.end(second);
        assert!(upgrade.begin(T0 + 4 + STALE_CALL_NS).is_ok());
    }

    #[test]
    fn staging_keeps_the_chunks_of_the_same_release_only() {
        let mut upgrade = StoredUpgrade::default();
        let release = manifest("1.0.0", &wasm());
        assert_eq!(upgrade.stage(7, release.clone()), 0);
        assert!(upgrade.clear_store);
        upgrade.clear_store = false;
        upgrade.uploaded(0);
        assert!(!upgrade.is_staged());
        assert_eq!(upgrade.stage(8, release.clone()), 1);
        assert_eq!(upgrade.action, Some(8));
        assert!(!upgrade.clear_store);
        upgrade.uploaded(1);
        assert!(upgrade.is_staged());

        let other = manifest("1.0.1", &wasm());
        assert_eq!(upgrade.stage(9, other), 0);
        assert!(upgrade.clear_store);
        assert_eq!(
            upgrade.state,
            UpgradeState::Staging {
                version: "1.0.1".to_string(),
                chunks_uploaded: 0,
                chunks_total: 2,
            }
        );
    }

    fn installing() -> StoredUpgrade {
        let mut upgrade = StoredUpgrade::default();
        let execution = upgrade.begin(T0).unwrap();
        upgrade.stage(7, manifest("1.0.0", &wasm()));
        upgrade.clear_store = false;
        upgrade.uploaded(1);
        upgrade.installing(T0);
        upgrade.end(execution);
        upgrade
    }

    #[test]
    fn an_install_ends_installed_or_failed_at_its_deadline() {
        let mut upgrade = installing();
        assert!(upgrade.is_installing());
        assert_eq!(upgrade.begin(T0), Err(WalletError::UpgradeInProgress));
        assert_eq!(upgrade.wake_at(T0), Some(T0 + INSTALL_DEADLINE_NS));
        assert_eq!(
            upgrade.clone().check_deadline(T0 + INSTALL_DEADLINE_NS - 1),
            None
        );

        let mut done = upgrade.clone();
        assert_eq!(done.installed(), Some((Some(7), "1.0.0".to_string())));
        assert_eq!(done.state, UpgradeState::Idle);
        assert!(done.clear_store);
        assert_eq!(done.wake_at(T0 + 5), Some(T0 + 5));
        assert_eq!(done.installed(), None);

        let deadline = T0 + INSTALL_DEADLINE_NS;
        assert!(matches!(
            upgrade.view(deadline),
            UpgradeState::Failed { at_ns, .. } if at_ns == deadline
        ));
        assert!(upgrade.is_installing());
        assert_eq!(
            upgrade.check_deadline(deadline),
            Some((Some(7), "1.0.0".to_string()))
        );
        assert_eq!(
            upgrade.state,
            UpgradeState::Failed {
                version: "1.0.0".to_string(),
                reason: failure("1.0.0"),
                at_ns: deadline,
            }
        );
        assert!(upgrade.clear_store);
        assert_eq!(upgrade.check_deadline(deadline + 1), None);
        assert_eq!(upgrade.installed(), None);
        assert!(upgrade.begin(deadline).is_ok());
    }

    #[test]
    fn a_rejected_staging_is_dropped_but_not_an_install() {
        let mut upgrade = StoredUpgrade::default();
        upgrade.stage(7, manifest("1.0.0", &wasm()));
        upgrade.clear_store = false;
        assert!(!upgrade.drop_staging(8));
        assert_eq!(upgrade.wake_at(T0), None);
        assert!(upgrade.drop_staging(7));
        assert_eq!(upgrade.state, UpgradeState::Idle);
        assert_eq!(upgrade.wake_at(T0), Some(T0));

        let mut upgrade = installing();
        assert!(!upgrade.drop_staging(7));
        assert!(upgrade.is_installing());
    }

    #[test]
    fn the_store_is_not_cleared_while_a_release_stages() {
        let mut upgrade = StoredUpgrade::default();
        upgrade.stage(7, manifest("1.0.0", &wasm()));
        assert!(upgrade.clear_store);
        assert_eq!(upgrade.wake_at(T0), None);
    }
}
