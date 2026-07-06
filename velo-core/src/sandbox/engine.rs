//! WASM sandbox runner using `wasmtime`.
//!
//! Executes pre-compiled `.wasm` tool modules in a fully isolated environment
//! with no host file system or network access. Used for volatile scripts that
//! should not have direct OS side-effects.

use std::path::Path;
use tracing::{info, warn};
use wasmtime::{Config, Engine, Linker, Module, Store};

use crate::error::VeloError;

/// Wasmtime-based isolated execution environment.
pub struct WasmRunner {
    engine: Engine,
}

impl WasmRunner {
    /// Create a new `WasmRunner` with a Cranelift-compiled engine.
    pub fn new() -> Result<Self, VeloError> {
        let mut config = Config::new();
        config
            .cranelift_opt_level(wasmtime::OptLevel::Speed)
            .parallel_compilation(true)
            .wasm_bulk_memory(true);

        let engine = Engine::new(&config).map_err(|e| VeloError::Wasm(e.to_string()))?;

        Ok(Self { engine })
    }

    /// Execute a WASM module from bytes.
    ///
    /// The module must export `velo_run(ptr: i32, len: i32) -> i32`.
    /// Input is written as a JSON string into WASM linear memory via `__alloc`.
    /// Returns the output string from WASM linear memory.
    ///
    /// # Safety
    /// The WASM runtime is fully sandboxed — no host access is granted.
    pub async fn run_bytes(
        &self,
        wasm_bytes: &[u8],
        input_json: &str,
    ) -> Result<String, VeloError> {
        let engine = self.engine.clone();
        let wasm_bytes = wasm_bytes.to_vec();
        let input = input_json.to_string();

        // Run on a blocking thread because wasmtime is synchronous
        tokio::task::spawn_blocking(move || Self::exec_sync(&engine, &wasm_bytes, &input))
            .await
            .map_err(|e| VeloError::Wasm(e.to_string()))?
    }

    /// Execute a WASM module from a file path.
    pub async fn run_file(&self, wasm_path: &Path, input_json: &str) -> Result<String, VeloError> {
        let bytes = tokio::fs::read(wasm_path)
            .await
            .map_err(VeloError::FileOp)?;
        self.run_bytes(&bytes, input_json).await
    }

    fn exec_sync(
        engine: &Engine,
        wasm_bytes: &[u8],
        input_json: &str,
    ) -> Result<String, VeloError> {
        info!("Compiling WASM module ({} bytes)", wasm_bytes.len());

        let module = Module::new(engine, wasm_bytes)
            .map_err(|e| VeloError::Wasm(format!("Compile error: {e}")))?;

        // No imports allowed — fully isolated
        let linker: Linker<()> = Linker::new(engine);
        let mut store = Store::new(engine, ());

        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| VeloError::Wasm(format!("Instantiate error: {e}")))?;

        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| VeloError::Wasm("No `memory` export".into()))?;

        // Write input JSON into WASM linear memory
        let input_bytes = input_json.as_bytes();
        let input_len = input_bytes.len() as i32;

        let input_ptr = match instance.get_typed_func::<i32, i32>(&mut store, "__alloc") {
            Ok(alloc_fn) => alloc_fn
                .call(&mut store, input_len)
                .map_err(|e| VeloError::Wasm(format!("__alloc failed: {e}")))?,
            Err(_) => {
                warn!("WASM module does not export `__alloc`; writing input at offset 0");
                0
            }
        };

        let mem = memory.data_mut(&mut store);
        let end = input_ptr as usize + input_len as usize;
        if end > mem.len() {
            return Err(VeloError::Wasm("Input exceeds memory bounds".into()));
        }
        mem[input_ptr as usize..end].copy_from_slice(input_bytes);

        // The module must export `velo_run: (ptr: i32, len: i32) -> i32`
        let run_fn = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, "velo_run")
            .map_err(|e| VeloError::Wasm(format!("Export `velo_run` not found: {e}")))?;

        let result_ptr = run_fn
            .call(&mut store, (input_ptr, input_len))
            .map_err(|e| VeloError::Wasm(format!("Execution error: {e}")))?;

        let mem_data = memory.data(&store);

        // Convention: result_ptr points to a u32 length followed by UTF-8 bytes
        if result_ptr < 0 || result_ptr as usize + 4 > mem_data.len() {
            return Err(VeloError::Wasm("Invalid result pointer".into()));
        }

        let ptr = result_ptr as usize;
        let len_bytes: [u8; 4] = mem_data[ptr..ptr + 4]
            .try_into()
            .map_err(|_| VeloError::Wasm("Failed to read result length".into()))?;
        let len = u32::from_le_bytes(len_bytes) as usize;

        if ptr + 4 + len > mem_data.len() {
            return Err(VeloError::Wasm("Result exceeds memory bounds".into()));
        }

        let output = std::str::from_utf8(&mem_data[ptr + 4..ptr + 4 + len])
            .map_err(|e| VeloError::Wasm(format!("Invalid UTF-8 in result: {e}")))?
            .to_string();

        info!(
            "WASM module executed successfully ({} bytes output)",
            output.len()
        );
        Ok(output)
    }
}

impl Default for WasmRunner {
    fn default() -> Self {
        Self::new().unwrap_or_else(|e| {
            tracing::warn!("WasmRunner::default failed, using fallback: {e}");
            let config = Config::new();
            let engine = Engine::new(&config).expect("Wasmtime engine creation failed in fallback");
            Self { engine }
        })
    }
}
