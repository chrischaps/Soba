//! Who signed a Windows executable, so an update can't come from someone
//! else. A checksum proves a download arrived as published; a signature says
//! who published it.
//!
//! Releases are signed through SignPath Foundation's open-source programme,
//! which signs as itself. If Soba moves to its own certificate, both
//! names stay accepted for a release, so either can update to the other.

use std::path::Path;

/// The publishers a Soba release can be signed by.
pub const PUBLISHERS: &[&str] = &["SignPath Foundation", "Chris Chappelear"];

/// An executable's Authenticode signature.
#[cfg_attr(not(windows), allow(dead_code))] // only Windows reads a signature
#[derive(Clone, Debug, PartialEq)]
pub enum Signature {
    /// Signed, and the signature checks out, by this publisher.
    Signed(String),
    Unsigned,
    /// Signed, but the signature doesn't hold: the status Windows gives.
    Broken(String),
    /// Couldn't tell (PowerShell wouldn't run).
    Unknown,
}

/// Whether `new` may replace `current`, going by their signatures: once a
/// copy is signed, only a signature from one of [`PUBLISHERS`] replaces it.
pub fn allowed(current: &Signature, new: &Signature) -> Result<(), String> {
    let ours = |publisher: &str| PUBLISHERS.contains(&publisher);
    match (current, new) {
        (_, Signature::Broken(status)) => Err(format!("The new version's signature doesn't hold ({status}), so it wasn't installed.")),
        (_, Signature::Signed(publisher)) if !ours(publisher) => {
            Err(format!("The new version is signed by {publisher}, not Soba's publisher, so it wasn't installed."))
        }
        (_, Signature::Signed(_)) => Ok(()),
        (Signature::Signed(publisher), _) => {
            Err(format!("The new version isn't signed by {publisher}, as this one is, so it wasn't installed."))
        }
        _ => Ok(()),
    }
}

/// Checks that `new` may replace `current`.
pub fn check(current: &Path, new: &Path) -> Result<(), String> {
    if !cfg!(windows) {
        return Ok(());
    }
    allowed(&read(current), &read(new))
}

/// Reads a file's signature through PowerShell's `Get-AuthenticodeSignature`.
#[cfg(windows)]
pub fn read(path: &Path) -> Signature {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // The path goes in through the environment, so nothing needs quoting
    let script = "$s = Get-AuthenticodeSignature -LiteralPath $env:SOBA_SIGNED_FILE; \
                  Write-Output $s.Status; Write-Output $s.SignerCertificate.Subject";
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("SOBA_SIGNED_FILE", path)
        // Windows PowerShell can't load its own modules with PowerShell 7's path
        .env_remove("PSModulePath")
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match output {
        Ok(out) if out.status.success() => parse(&String::from_utf8_lossy(&out.stdout)),
        Ok(out) => {
            eprintln!("update: couldn't read the signature of {}: {}", path.display(), String::from_utf8_lossy(&out.stderr).trim());
            Signature::Unknown
        }
        Err(e) => {
            eprintln!("update: couldn't run PowerShell to read a signature: {e}");
            Signature::Unknown
        }
    }
}

#[cfg(not(windows))]
pub fn read(_path: &Path) -> Signature {
    Signature::Unknown
}

/// `Get-AuthenticodeSignature`'s status, then the signer's subject.
#[cfg_attr(not(windows), allow(dead_code))] // only Windows reads a signature
fn parse(output: &str) -> Signature {
    let mut lines = output.lines().map(str::trim);
    let status = lines.next().unwrap_or_default();
    let subject = lines.next().unwrap_or_default();
    match status {
        "Valid" => Signature::Signed(common_name(subject).unwrap_or(subject).to_string()),
        "NotSigned" => Signature::Unsigned,
        "" => Signature::Unknown,
        other => Signature::Broken(other.to_string()),
    }
}

/// The `CN=` of a certificate subject.
#[cfg_attr(not(windows), allow(dead_code))] // only Windows reads a signature
fn common_name(subject: &str) -> Option<&str> {
    subject.split(',').map(str::trim).find_map(|part| part.strip_prefix("CN=")).map(|cn| cn.trim_matches('"'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Signature::*;

    #[test]
    fn powershell_output_is_read() {
        assert_eq!(
            parse("Valid\r\nCN=SignPath Foundation, O=SignPath Foundation, L=Lewes, S=Delaware, C=US\r\n"),
            Signed("SignPath Foundation".into())
        );
        assert_eq!(parse("NotSigned\r\n\r\n"), Unsigned);
        assert_eq!(parse("HashMismatch\r\nCN=Someone\r\n"), Broken("HashMismatch".into()));
        assert_eq!(parse(""), Unknown);
    }

    #[test]
    fn a_signed_copy_takes_only_a_signed_update_from_its_publishers() {
        let signpath = Signed("SignPath Foundation".into());
        let chris = Signed("Chris Chappelear".into());
        let stranger = Signed("Someone Else Ltd".into());

        assert!(allowed(&signpath, &signpath).is_ok());
        assert!(allowed(&signpath, &chris).is_ok(), "the switch to Azure signing");
        assert!(allowed(&signpath, &Unsigned).is_err());
        assert!(allowed(&signpath, &Unknown).is_err());
        assert!(allowed(&signpath, &stranger).is_err());
        assert!(allowed(&signpath, &Broken("HashMismatch".into())).is_err());

        // Today's unsigned releases update to unsigned or signed ones
        assert!(allowed(&Unsigned, &Unsigned).is_ok());
        assert!(allowed(&Unsigned, &signpath).is_ok());
        assert!(allowed(&Unsigned, &stranger).is_err());
        assert!(allowed(&Unknown, &Unknown).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn this_test_binary_reads_as_unsigned() {
        let exe = std::env::current_exe().unwrap();
        assert!(matches!(read(&exe), Unsigned | Unknown));
    }
}
