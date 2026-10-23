//! What the IC's `canister_info` says about a canister, and the check that a
//! controller was installed once, fresh, with the module its owners
//! approved.
//!
//! A module hash alone does not say what state a canister holds: whoever
//! controlled it before it gave up its controllers could have loaded a
//! snapshot with crafted memory, or uninstalled and reinstalled it. The
//! controller refuses to be upgraded to its own module, so the only way to
//! run it with state its own `init` did not write is a snapshot. The IC
//! records every code deployment and snapshot load, so a controller whose
//! whole history is one install of the approved module, its creation and
//! changes of its controllers holds only what that install wrote.

use candid::{CandidType, Principal, Reserved};
use serde::Deserialize;
use serde_bytes::ByteBuf;

/// The most changes `canister_info` is asked for. The IC keeps at least 20.
pub const MAX_CHANGES: u64 = 20;

/// The fields of `canister_info`'s answer this crate reads.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CanisterInfo {
    pub total_num_changes: u64,
    pub recent_changes: Vec<Change>,
    pub module_hash: Option<ByteBuf>,
    pub controllers: Vec<Principal>,
}

/// One change; its time and the canister's version are not read.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub origin: ChangeOrigin,
    pub details: ChangeDetails,
}

/// Who made a change. A canister that passes its own version with a call
/// to the management canister finds it here, which tells that call's
/// change from any other.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ChangeOrigin {
    #[serde(rename = "from_user")]
    FromUser { user_id: Principal },
    #[serde(rename = "from_canister")]
    FromCanister {
        canister_id: Principal,
        canister_version: Option<u64>,
    },
}

/// A kind of change the IC adds later fails to decode, so the check fails
/// closed.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ChangeDetails {
    #[serde(rename = "creation")]
    Creation(Reserved),
    #[serde(rename = "code_uninstall")]
    CodeUninstall,
    #[serde(rename = "code_deployment")]
    CodeDeployment {
        mode: DeploymentMode,
        module_hash: ByteBuf,
    },
    #[serde(rename = "load_snapshot")]
    LoadSnapshot(Reserved),
    #[serde(rename = "controllers_change")]
    ControllersChange(Reserved),
    #[serde(rename = "rename_canister")]
    RenameCanister(Reserved),
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentMode {
    #[serde(rename = "install")]
    Install,
    #[serde(rename = "reinstall")]
    Reinstall,
    #[serde(rename = "upgrade")]
    Upgrade,
}

/// Checks that `info`, asked with `MAX_CHANGES` changes, describes a
/// canister that runs `module_hash`, has no controller, and whose whole
/// history is its creation, one install of `module_hash`, and changes of its
/// controllers.
pub fn check_fresh_controller(info: &CanisterInfo, module_hash: &[u8]) -> Result<(), String> {
    match &info.module_hash {
        Some(hash) if hash.as_slice() == module_hash => {}
        Some(hash) => {
            return Err(format!(
                "it runs module {}, not the approved {}",
                hex(hash),
                hex(module_hash)
            ));
        }
        None => return Err("it runs no module".to_string()),
    }
    if !info.controllers.is_empty() {
        let controllers: Vec<String> = info.controllers.iter().map(Principal::to_text).collect();
        return Err(format!(
            "it has controllers ({}); it must have none",
            controllers.join(", ")
        ));
    }
    if info.recent_changes.len() as u64 != info.total_num_changes {
        return Err(format!(
            "the IC holds {} of its {} changes, so its history cannot be checked",
            info.recent_changes.len(),
            info.total_num_changes
        ));
    }
    let mut installs = 0;
    for change in &info.recent_changes {
        match &change.details {
            ChangeDetails::Creation(_) | ChangeDetails::ControllersChange(_) => {}
            ChangeDetails::CodeDeployment {
                mode: DeploymentMode::Install,
                module_hash: hash,
            } if hash.as_slice() == module_hash => installs += 1,
            ChangeDetails::CodeDeployment { .. } => {
                return Err(
                    "its code was deployed other than by one install of the approved \
                     module"
                        .to_string(),
                );
            }
            ChangeDetails::CodeUninstall => return Err("its code was uninstalled".to_string()),
            ChangeDetails::LoadSnapshot(_) => {
                return Err("a snapshot was loaded into it".to_string());
            }
            ChangeDetails::RenameCanister(_) => return Err("it was renamed".to_string()),
        }
    }
    if installs != 1 {
        return Err(format!(
            "its history holds {installs} installs of the approved module, not one"
        ));
    }
    Ok(())
}

/// The change at position `index` of the canister's history, counting from
/// 0 in the order the IC made them, if `info` holds it.
pub fn change_at(info: &CanisterInfo, index: u64) -> Option<&Change> {
    let first = info
        .total_num_changes
        .checked_sub(info.recent_changes.len() as u64)?;
    let offset = usize::try_from(index.checked_sub(first)?).ok()?;
    info.recent_changes.get(offset)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use candid::{Decode, Encode};

    const MODULE: [u8; 32] = [7; 32];

    fn origin() -> ChangeOrigin {
        ChangeOrigin::FromUser {
            user_id: Principal::anonymous(),
        }
    }

    fn deployment(mode: DeploymentMode, hash: [u8; 32]) -> Change {
        Change {
            origin: origin(),
            details: ChangeDetails::CodeDeployment {
                mode,
                module_hash: ByteBuf::from(hash.to_vec()),
            },
        }
    }

    fn other(details: ChangeDetails) -> Change {
        Change {
            origin: origin(),
            details,
        }
    }

    fn fresh() -> CanisterInfo {
        let changes = vec![
            other(ChangeDetails::Creation(Reserved)),
            deployment(DeploymentMode::Install, MODULE),
            other(ChangeDetails::ControllersChange(Reserved)),
        ];
        CanisterInfo {
            total_num_changes: changes.len() as u64,
            recent_changes: changes,
            module_hash: Some(ByteBuf::from(MODULE.to_vec())),
            controllers: vec![],
        }
    }

    #[test]
    fn a_fresh_controller_passes() {
        assert_eq!(check_fresh_controller(&fresh(), &MODULE), Ok(()));
    }

    #[test]
    fn anything_but_one_fresh_install_fails() {
        let mut info = fresh();
        info.module_hash = Some(ByteBuf::from(vec![8; 32]));
        assert!(check_fresh_controller(&info, &MODULE).is_err());

        let mut info = fresh();
        info.controllers = vec![Principal::anonymous()];
        assert!(check_fresh_controller(&info, &MODULE).is_err());

        let mut info = fresh();
        info.total_num_changes += 1;
        assert!(check_fresh_controller(&info, &MODULE).is_err());

        for change in [
            other(ChangeDetails::LoadSnapshot(Reserved)),
            other(ChangeDetails::CodeUninstall),
            other(ChangeDetails::RenameCanister(Reserved)),
            deployment(DeploymentMode::Install, MODULE),
            deployment(DeploymentMode::Reinstall, MODULE),
            deployment(DeploymentMode::Upgrade, MODULE),
            deployment(DeploymentMode::Install, [8; 32]),
        ] {
            let mut info = fresh();
            info.recent_changes.push(change.clone());
            info.total_num_changes += 1;
            assert!(
                check_fresh_controller(&info, &MODULE).is_err(),
                "{change:?} passed"
            );
        }

        let mut info = fresh();
        info.recent_changes.remove(1);
        info.total_num_changes -= 1;
        assert!(check_fresh_controller(&info, &MODULE).is_err());
    }

    #[test]
    fn a_change_is_found_by_its_position_in_the_history() {
        let mut info = fresh();
        assert_eq!(
            change_at(&info, 1),
            Some(&deployment(DeploymentMode::Install, MODULE))
        );
        assert_eq!(change_at(&info, 3), None);
        // Only the last two of five changes.
        info.total_num_changes = 5;
        info.recent_changes.remove(0);
        assert_eq!(change_at(&info, 2), None);
        assert_eq!(
            change_at(&info, 4),
            Some(&other(ChangeDetails::ControllersChange(Reserved)))
        );
        info.total_num_changes = 1;
        assert_eq!(change_at(&info, 0), None);
    }

    /// The management canister's own types, as `ic.did` states them,
    /// decode into this crate's narrower ones.
    #[test]
    fn decodes_the_management_canister_answer() {
        #[derive(CandidType, Deserialize)]
        enum Origin {
            #[serde(rename = "from_user")]
            FromUser { user_id: Principal },
            #[serde(rename = "from_canister")]
            FromCanister {
                canister_id: Principal,
                canister_version: Option<u64>,
            },
        }
        #[derive(CandidType, Deserialize)]
        enum Source {
            #[serde(rename = "metadata_upload")]
            MetadataUpload(Reserved),
        }
        #[derive(CandidType, Deserialize)]
        enum Mode {
            #[serde(rename = "install")]
            Install,
        }
        #[derive(CandidType, Deserialize)]
        enum Details {
            #[serde(rename = "creation")]
            Creation {
                controllers: Vec<Principal>,
                environment_variables_hash: Option<ByteBuf>,
            },
            #[serde(rename = "code_deployment")]
            CodeDeployment { mode: Mode, module_hash: ByteBuf },
            #[serde(rename = "load_snapshot")]
            LoadSnapshot {
                from_canister_id: Option<Principal>,
                snapshot_id: ByteBuf,
                canister_version: u64,
                taken_at_timestamp: u64,
                source: Source,
            },
            #[serde(rename = "controllers_change")]
            ControllersChange { controllers: Vec<Principal> },
        }
        #[derive(CandidType, Deserialize)]
        struct FullChange {
            timestamp_nanos: u64,
            canister_version: u64,
            origin: Origin,
            details: Details,
        }
        #[derive(CandidType, Deserialize)]
        struct Answer {
            total_num_changes: u64,
            recent_changes: Vec<FullChange>,
            module_hash: Option<ByteBuf>,
            controllers: Vec<Principal>,
        }
        let change = |details| FullChange {
            timestamp_nanos: 1,
            canister_version: 2,
            origin: Origin::FromUser {
                user_id: Principal::anonymous(),
            },
            details,
        };
        let from_canister = |details| FullChange {
            origin: Origin::FromCanister {
                canister_id: Principal::management_canister(),
                canister_version: Some(7),
            },
            ..change(details)
        };
        let answer = Answer {
            total_num_changes: 4,
            recent_changes: vec![
                change(Details::Creation {
                    controllers: vec![Principal::anonymous()],
                    environment_variables_hash: None,
                }),
                from_canister(Details::CodeDeployment {
                    mode: Mode::Install,
                    module_hash: ByteBuf::from(MODULE.to_vec()),
                }),
                change(Details::ControllersChange {
                    controllers: vec![],
                }),
                change(Details::LoadSnapshot {
                    from_canister_id: None,
                    snapshot_id: ByteBuf::from(vec![1, 2]),
                    canister_version: 3,
                    taken_at_timestamp: 4,
                    source: Source::MetadataUpload(Reserved),
                }),
            ],
            module_hash: Some(ByteBuf::from(MODULE.to_vec())),
            controllers: vec![],
        };
        let bytes = Encode!(&answer).unwrap();
        let info = Decode!(&bytes, CanisterInfo).unwrap();
        assert_eq!(info.recent_changes.len(), 4);
        assert_eq!(
            info.recent_changes[1],
            Change {
                origin: ChangeOrigin::FromCanister {
                    canister_id: Principal::management_canister(),
                    canister_version: Some(7)
                },
                details: ChangeDetails::CodeDeployment {
                    mode: DeploymentMode::Install,
                    module_hash: ByteBuf::from(MODULE.to_vec())
                }
            }
        );
        assert_eq!(
            check_fresh_controller(&info, &MODULE),
            Err("a snapshot was loaded into it".to_string())
        );
    }
}
