//! Makes, signs and checks wallet release manifests. It never talks to the
//! network: icp-cli sends what it writes (see the README).

use release::{ReleaseManifest, Signers, sha256};
use release_tool::{candid_blob, chunk_args, from_hex, from_text, hex, read_key, to_text};
use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

const USAGE: &str = "\
Usage:
  release-tool manifest --wasm FILE --version VERSION --notes-url URL
                        [--tested-from MODULE_HASH]... [--published-at-ns NS]
      Prints the unsigned manifest of a Wasm as Candid text.
  release-tool sign --key KEY.pem [--wasm FILE] MANIFEST
      Adds the key's signature to the manifest file, in place of one it made
      before. With --wasm, first checks that the manifest names that Wasm.
  release-tool verify --signer PUBLIC_KEY... --threshold N [--wasm FILE] MANIFEST
      Checks the manifest's form and signatures, and with --wasm its chunks.
  release-tool public-key [--candid] --key KEY.pem
      Prints the key's public key in hex, or as a Candid blob.
  release-tool chunks --wasm FILE --version VERSION --out DIR
      Writes the arguments of publish_chunk for each chunk, as DIR/chunk-N.bin.

Keys are Ed25519 private keys in PKCS#8 PEM, as written by
  openssl genpkey -algorithm ed25519 -out KEY.pem
Public keys and module hashes are hex.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(message)) => {
            eprintln!("release-tool: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Refused(message)) => {
            eprintln!("release-tool: {message}");
            ExitCode::FAILURE
        }
    }
}

enum Failure {
    Usage(String),
    Refused(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::Refused(message)
    }
}

fn run(args: &[String]) -> Result<(), Failure> {
    let Some((command, rest)) = args.split_first() else {
        return Err(Failure::Usage("no command".to_string()));
    };
    let options = Options::parse(rest)?;
    match command.as_str() {
        "manifest" => manifest(options),
        "sign" => sign(options),
        "verify" => verify(options),
        "public-key" => public_key(options),
        "chunks" => write_chunks(options),
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(Failure::Usage(format!("unknown command {other}"))),
    }
}

fn manifest(mut options: Options) -> Result<(), Failure> {
    let wasm = read(&options.one("--wasm")?)?;
    let version = options.one("--version")?;
    let notes_url = options.one("--notes-url")?;
    let tested_from = options
        .all("--tested-from")
        .iter()
        .map(|text| {
            from_hex(text)?
                .try_into()
                .map_err(|_| format!("module hash {text} is not 32 bytes"))
        })
        .collect::<Result<Vec<[u8; 32]>, String>>()?;
    let published_at_ns = match options.optional("--published-at-ns")? {
        Some(ns) => ns
            .parse()
            .map_err(|_| Failure::Usage(format!("--published-at-ns {ns} is not a number")))?,
        None => SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_nanos() as u64,
    };
    options.done(0)?;
    let manifest = ReleaseManifest::new(version, &wasm, tested_from, notes_url, published_at_ns);
    manifest.check_form()?;
    print!("{}", to_text(&manifest));
    Ok(())
}

fn sign(mut options: Options) -> Result<(), Failure> {
    let key = read_key(&read_text(&options.one("--key")?)?)?;
    let wasm = options.optional("--wasm")?;
    let [path] = options.done(1)?.try_into().expect("one argument");
    let mut manifest = read_manifest(&path)?;
    if let Some(wasm) = wasm {
        check_wasm(&manifest, &wasm)?;
    }
    eprintln!("{}", describe(&manifest));
    manifest.sign(&key);
    manifest
        .check_form()
        .map_err(|e| format!("{path} cannot take this signature: {e}"))?;
    // A new file renamed over the old one, so a failed write leaves the
    // manifest as it was.
    let new = format!("{path}.new");
    std::fs::write(&new, to_text(&manifest)).map_err(|e| format!("{new}: {e}"))?;
    std::fs::rename(&new, &path).map_err(|e| format!("{path}: {e}"))?;
    eprintln!(
        "Signed by {}. Signatures on the manifest: {}.",
        hex(key.verifying_key().as_bytes()),
        manifest.signatures.len()
    );
    Ok(())
}

fn verify(mut options: Options) -> Result<(), Failure> {
    let keys = options
        .all("--signer")
        .iter()
        .map(|text| from_hex(text))
        .collect::<Result<Vec<_>, _>>()?;
    let threshold = options.one("--threshold")?;
    let threshold: u8 = threshold
        .parse()
        .map_err(|_| Failure::Usage(format!("--threshold {threshold} is not a number")))?;
    let wasm = options.optional("--wasm")?;
    let [path] = options.done(1)?.try_into().expect("one argument");
    let signers = Signers::new(&keys, threshold)?;
    let manifest = read_manifest(&path)?;
    println!("{}", describe(&manifest));
    let have = signers.count(&manifest);
    println!(
        "Signed by {have} of the {} keys; {threshold} needed.",
        keys.len()
    );
    if let Some(wasm) = wasm {
        check_wasm(&manifest, &wasm)?;
        println!("Its chunks are the Wasm in {wasm}.");
    }
    signers.check(&manifest).map_err(|e| {
        format!(
            "{path} has {} of the {} signatures it needs",
            e.have, e.need
        )
    })?;
    Ok(())
}

fn public_key(mut options: Options) -> Result<(), Failure> {
    let candid = options.flag("--candid");
    let key = read_key(&read_text(&options.one("--key")?)?)?;
    options.done(0)?;
    let public = key.verifying_key().to_bytes();
    if candid {
        println!("{}", candid_blob(&public));
    } else {
        println!("{}", hex(&public));
    }
    Ok(())
}

fn write_chunks(mut options: Options) -> Result<(), Failure> {
    let wasm = read(&options.one("--wasm")?)?;
    let version = options.one("--version")?;
    let out = options.one("--out")?;
    options.done(0)?;
    release::check_version(&version)?;
    std::fs::create_dir_all(&out).map_err(|e| format!("{out}: {e}"))?;
    let args = chunk_args(&version, &wasm);
    for (index, bytes) in args.iter().enumerate() {
        let path = Path::new(&out).join(format!("chunk-{index}.bin"));
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    eprintln!("Wrote {} chunks to {out}.", args.len());
    Ok(())
}

/// What a signer should check before signing.
fn describe(manifest: &ReleaseManifest) -> String {
    format!(
        "Release {}: Wasm of {} bytes with SHA-256 {}, in {} chunks; notes at {}; \
         tested to upgrade from {}; published at {} ns. Signing hash {}.",
        manifest.version,
        manifest.wasm_size,
        hex(&manifest.wasm_sha256),
        manifest.chunk_hashes.len(),
        manifest.notes_url,
        match manifest.tested_from.len() {
            1 => "1 module hash".to_string(),
            n => format!("{n} module hashes"),
        },
        manifest.published_at_ns,
        hex(&manifest.signing_hash())
    )
}

fn check_wasm(manifest: &ReleaseManifest, path: &str) -> Result<(), String> {
    let wasm = read(path)?;
    manifest.check_wasm(&wasm).map_err(|e| {
        format!(
            "the manifest does not name the Wasm in {path} ({} bytes, SHA-256 {}): {e:?}",
            wasm.len(),
            hex(&sha256(&wasm))
        )
    })
}

fn read_manifest(path: &str) -> Result<ReleaseManifest, String> {
    let manifest = from_text(&read_text(path)?).map_err(|e| format!("{path}: {e}"))?;
    manifest.check_form().map_err(|e| format!("{path}: {e}"))?;
    Ok(manifest)
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{path}: {e}"))
}

fn read_text(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

/// `--name value` options, some repeated, `--name` flags, and arguments.
struct Options {
    values: HashMap<String, Vec<String>>,
    flags: Vec<String>,
    arguments: Vec<String>,
}

const FLAGS: [&str; 1] = ["--candid"];

impl Options {
    fn parse(args: &[String]) -> Result<Options, Failure> {
        let mut options = Options {
            values: HashMap::new(),
            flags: Vec::new(),
            arguments: Vec::new(),
        };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if FLAGS.contains(&arg.as_str()) {
                options.flags.push(arg.clone());
            } else if arg.starts_with("--") {
                let value = args
                    .next()
                    .ok_or_else(|| Failure::Usage(format!("{arg} needs a value")))?;
                options
                    .values
                    .entry(arg.clone())
                    .or_default()
                    .push(value.clone());
            } else {
                options.arguments.push(arg.clone());
            }
        }
        Ok(options)
    }

    fn all(&mut self, name: &str) -> Vec<String> {
        self.values.remove(name).unwrap_or_default()
    }

    fn optional(&mut self, name: &str) -> Result<Option<String>, Failure> {
        let mut values = self.all(name);
        match values.len() {
            0 => Ok(None),
            1 => Ok(values.pop()),
            _ => Err(Failure::Usage(format!("{name} given more than once"))),
        }
    }

    fn one(&mut self, name: &str) -> Result<String, Failure> {
        self.optional(name)?
            .ok_or_else(|| Failure::Usage(format!("{name} is needed")))
    }

    fn flag(&mut self, name: &str) -> bool {
        let given = self.flags.iter().any(|flag| flag == name);
        self.flags.retain(|flag| flag != name);
        given
    }

    /// Refuses options the command did not read, and returns its `count`
    /// arguments.
    fn done(self, count: usize) -> Result<Vec<String>, Failure> {
        if let Some(name) = self.values.keys().chain(&self.flags).next() {
            return Err(Failure::Usage(format!("unknown option {name}")));
        }
        if self.arguments.len() != count {
            return Err(Failure::Usage(format!(
                "expected {count} arguments, got {}",
                self.arguments.len()
            )));
        }
        Ok(self.arguments)
    }
}
