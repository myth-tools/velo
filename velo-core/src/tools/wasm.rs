use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sandbox::WasmRunner;
use crate::tools::{exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct RunWasmArgs {
    pub wasm_bytes_b64: Option<String>,
    pub wasm_path: Option<String>,
    pub input_json: Option<String>,
}

impl ToolInputT for RunWasmArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"wasm_bytes_b64":{"type":"string","description":"Base64-encoded WASM binary. Provide either this or wasm_path."},"wasm_path":{"type":"string","description":"File path to a .wasm file on disk. Provide either this or wasm_bytes_b64."},"input_json":{"type":"string","description":"Optional JSON input string to pass to the WASM module. Defaults to '{}'."}}}"#
    }
}

#[tool(name = "run_wasm", description = "Execute a WASM (WebAssembly) module in a fully isolated sandbox. The module must export `velo_run(ptr: i32, len: i32) -> i32` and optionally `__alloc(i32) -> i32`. Input is written to WASM linear memory as JSON. The module has NO host access — no filesystem, network, or OS interaction. BEST FOR: running untrusted or volatile code where isolation is critical.", input = RunWasmArgs)]
#[derive(Default, Clone)]
pub struct RunWasmTool;

#[async_trait]
impl ToolRuntime for RunWasmTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: RunWasmArgs = serde_json::from_value(args)?;
        let runner = WasmRunner::new().map_err(|e| exec_err(format!("Wasm init: {e}")))?;
        let input = a.input_json.unwrap_or_else(|| "{}".to_string());

        let result = if let Some(b64) = &a.wasm_bytes_b64 {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| exec_err(format!("Base64 decode: {e}")))?;
            runner.run_bytes(&bytes, &input).await
        } else if let Some(path) = &a.wasm_path {
            runner.run_file(std::path::Path::new(path), &input).await
        } else {
            return Err(exec_err("Either wasm_bytes_b64 or wasm_path is required"));
        };

        match result {
            Ok(output) => Ok(ToolOutput::ok(output).into()),
            Err(e) => Err(exec_err(format!("Wasm error: {e}"))),
        }
    }
}
