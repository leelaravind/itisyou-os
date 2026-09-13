//! `keytool` — the OFFLINE half of the package-signing key hierarchy (V0.9).
//!
//! Everything that needs the root's private key happens here, on the owner's
//! machine, never in the build and never in CI: issuing a signing-key
//! certificate and signing a revocation list. The outputs are PUBLIC data
//! (certificates, revocation lists, the root's public key) and are committed
//! under `keys/`; the root seed itself stays outside the source tree.
//!
//! A seed file is exactly 32 raw bytes. The same codec the kernel verifies
//! with (`kernel_core::trust`) produces every byte here, so the two cannot
//! drift.
//!
//! ```text
//! keytool public  <seed-file>                         print the public key (hex)
//! keytool cert    <root-seed> <out> <key-id> <signer-seed> <first-epoch>
//!                 <last-epoch> <scope|-> <label>       issue a certificate
//! keytool revoke  <root-seed> <out> <sequence> [key-id ...]
//! keytool check   <root-public-hex> <file>             verify a cert or list
//! ```

use kernel_core::{ed25519, trust};
use std::process::ExitCode;

fn read_seed(path: &str) -> Result<[u8; 32], String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    bytes
        .try_into()
        .map_err(|_| format!("{path}: a seed file must be exactly 32 bytes"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_hex32(text: &str) -> Result<[u8; 32], String> {
    let text = text.trim();
    if text.len() != 64 {
        return Err("expected 64 hex digits".into());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn num(text: &str) -> Result<u32, String> {
    text.parse().map_err(|_| format!("not a number: {text}"))
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("public") if args.len() == 2 => {
            println!("{}", hex(&ed25519::public_key(&read_seed(&args[1])?)));
            Ok(())
        }
        Some("cert") if args.len() == 9 => {
            let root = read_seed(&args[1])?;
            let signer = ed25519::public_key(&read_seed(&args[4])?);
            let scope = if args[7] == "-" { "" } else { &args[7] };
            let mut body = [0u8; trust::MAX_CERT_BODY];
            let n = trust::encode_cert_body(
                &mut body,
                num(&args[3])?,
                &signer,
                num(&args[5])?,
                num(&args[6])?,
                scope,
                &args[8],
            )
            .map_err(|e| e.name().to_string())?;
            let mut buf = [0u8; trust::CERT_CONTEXT.len() + trust::MAX_CERT_BODY];
            let sig = ed25519::sign(&root, trust::cert_signing_input(&body[..n], &mut buf));
            let mut out = body[..n].to_vec();
            out.extend_from_slice(&sig);
            // Refuse to write anything the kernel would not accept.
            trust::verify_certificate(&out, &ed25519::public_key(&root))
                .map_err(|e| e.name().to_string())?;
            std::fs::write(&args[2], &out).map_err(|e| e.to_string())?;
            println!(
                "certificate key_id={} signer={} scope={scope:?} -> {}",
                args[3],
                hex(&signer),
                args[2]
            );
            Ok(())
        }
        Some("revoke") if args.len() >= 4 => {
            let root = read_seed(&args[1])?;
            let ids = args[4..]
                .iter()
                .map(|a| num(a))
                .collect::<Result<Vec<_>, _>>()?;
            let mut body = [0u8; trust::MAX_REVOCATION_BODY];
            let n = trust::encode_revocation_body(&mut body, num(&args[3])?, &ids)
                .map_err(|e| e.name().to_string())?;
            let mut buf = [0u8; trust::REVOCATION_CONTEXT.len() + trust::MAX_REVOCATION_BODY];
            let sig = ed25519::sign(&root, trust::revocation_signing_input(&body[..n], &mut buf));
            let mut out = body[..n].to_vec();
            out.extend_from_slice(&sig);
            trust::verify_revocations(&out, &ed25519::public_key(&root))
                .map_err(|e| e.name().to_string())?;
            std::fs::write(&args[2], &out).map_err(|e| e.to_string())?;
            println!(
                "revocations sequence={} revoked={ids:?} -> {}",
                args[3], args[2]
            );
            Ok(())
        }
        Some("check") if args.len() == 3 => {
            let root = parse_hex32(&args[1])?;
            let bytes = std::fs::read(&args[2]).map_err(|e| e.to_string())?;
            if bytes.starts_with(trust::CERT_MAGIC) {
                let k =
                    trust::verify_certificate(&bytes, &root).map_err(|e| e.name().to_string())?;
                println!(
                    "OK certificate key_id={} epochs={}..={} scope={:?} label={}",
                    k.key_id,
                    k.first_epoch,
                    k.last_epoch,
                    k.scope(),
                    k.label()
                );
            } else {
                let r =
                    trust::verify_revocations(&bytes, &root).map_err(|e| e.name().to_string())?;
                println!(
                    "OK revocations sequence={} revoked={:?}",
                    r.sequence,
                    r.revoked()
                );
            }
            Ok(())
        }
        _ => Err("usage: keytool public|cert|revoke|check ... (see the source header)".into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("keytool: {e}");
            ExitCode::FAILURE
        }
    }
}
