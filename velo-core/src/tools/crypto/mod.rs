use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use base64::Engine;
use hmac::{Hmac, Mac};
use md5::Md5;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

use crate::tools::{exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct CryptoHashArgs {
    pub action: String,
    pub input: Option<String>,
    pub input_file: Option<String>,
    pub key: Option<String>,
    pub encoding: Option<String>,
}

impl ToolInputT for CryptoHashArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"action":{"type":"string","description":"Operation: 'base64_encode', 'base64_decode', 'sha256', 'sha512', 'sha1', 'md5', 'hmac_sha256', 'hmac_sha512'."},"input":{"type":"string","description":"Input string/bytes to process. Alternative to input_file (specify one or the other)."},"input_file":{"type":"string","description":"Path to file for file-based hashing. Alternative to input (specify one or the other). Useful for verifying file checksums."},"key":{"type":"string","description":"Secret key for HMAC operations (hmac_sha256, hmac_sha512). Required for those actions."},"encoding":{"type":"string","description":"Output format: 'hex' (default, lowercase hexadecimal) or 'base64'. For hashing actions only."}}}"#
    }
}

#[tool(name = "crypto_hash", description = "Cryptographic operations: base64 encode/decode, SHA-256/512/1, MD5 hashing, HMAC-SHA256/512. Supports string and file input. Output in hex (default) or base64. BEST FOR: encoding data, verifying checksums, computing file hashes, authenticating messages. Use the shell tool with openssl for more advanced crypto needs (encryption, signing, certificate handling).", input = CryptoHashArgs)]
#[derive(Default, Clone)]
pub struct CryptoHashTool;

#[async_trait]
impl ToolRuntime for CryptoHashTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: CryptoHashArgs = serde_json::from_value(args)?;
        let action = a.action.to_lowercase();
        let encoding = a.encoding.as_deref().unwrap_or("hex");
        let hex_out = encoding != "base64";

        let result = match action.as_str() {
            "base64_encode" => {
                let input = input_data(&a)?;
                base64::engine::general_purpose::STANDARD.encode(&input)
            }
            "base64_decode" => {
                let input = a
                    .input
                    .as_deref()
                    .ok_or_else(|| exec_err("input required for base64_decode"))?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(input)
                    .map_err(|e| exec_err(format!("Invalid base64: {e}")))?;
                // Try UTF-8 first, fall back to hex
                match String::from_utf8(bytes) {
                    Ok(s) => s,
                    Err(e) => {
                        let hex = hex::encode(e.into_bytes());
                        format!("(binary, hex: {hex})")
                    }
                }
            }
            "sha256" => {
                let data = input_data(&a)?;
                if hex_out {
                    hex_encode::<Sha256>(&data)
                } else {
                    base64_encode::<Sha256>(&data)
                }
            }
            "sha512" => {
                let data = input_data(&a)?;
                if hex_out {
                    hex_encode::<Sha512>(&data)
                } else {
                    base64_encode::<Sha512>(&data)
                }
            }
            "sha1" => {
                let data = input_data(&a)?;
                if hex_out {
                    hex_encode::<Sha1>(&data)
                } else {
                    base64_encode::<Sha1>(&data)
                }
            }
            "md5" => {
                let data = input_data(&a)?;
                let digest = Md5::digest(&data);
                if hex_out {
                    format!("{digest:x}")
                } else {
                    base64::engine::general_purpose::STANDARD.encode(digest)
                }
            }
            "hmac_sha256" => {
                let data = input_data(&a)?;
                let key = a
                    .key
                    .as_deref()
                    .ok_or_else(|| exec_err("key required for hmac_sha256"))?;
                let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes())
                    .map_err(|e| exec_err(format!("Invalid HMAC key: {e}")))?;
                mac.update(&data);
                let result = mac.finalize().into_bytes();
                if hex_out {
                    hex::encode(result)
                } else {
                    base64::engine::general_purpose::STANDARD.encode(result)
                }
            }
            "hmac_sha512" => {
                let data = input_data(&a)?;
                let key = a
                    .key
                    .as_deref()
                    .ok_or_else(|| exec_err("key required for hmac_sha512"))?;
                let mut mac = Hmac::<Sha512>::new_from_slice(key.as_bytes())
                    .map_err(|e| exec_err(format!("Invalid HMAC key: {e}")))?;
                mac.update(&data);
                let result = mac.finalize().into_bytes();
                if hex_out {
                    hex::encode(result)
                } else {
                    base64::engine::general_purpose::STANDARD.encode(result)
                }
            }
            other => return Err(exec_err(format!("Unknown action '{other}'"))),
        };

        Ok(ToolOutput::ok(format!("{action}: {result}")).into())
    }
}

fn input_data(a: &CryptoHashArgs) -> Result<Vec<u8>, ToolCallError> {
    if let Some(ref path) = a.input_file {
        let data =
            std::fs::read(path).map_err(|e| exec_err(format!("Cannot read file '{path}': {e}")))?;
        Ok(data)
    } else if let Some(ref input) = a.input {
        Ok(input.as_bytes().to_vec())
    } else {
        Err(exec_err("Either 'input' or 'input_file' is required"))
    }
}

fn hex_encode<D: Digest>(data: &[u8]) -> String {
    let mut hasher = D::new();
    hasher.update(data);
    let result = hasher.finalize();
    hex::encode(result)
}

fn base64_encode<D: Digest>(data: &[u8]) -> String {
    let mut hasher = D::new();
    hasher.update(data);
    let result = hasher.finalize();
    base64::engine::general_purpose::STANDARD.encode(result)
}
