use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use base64::Engine;
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::tools::{exec_err, ScrapePageArgs, ToolOutput};

type SharedBrowser = Arc<Mutex<Option<(Browser, Arc<Page>)>>>;

static BROWSER: OnceLock<SharedBrowser> = OnceLock::new();

fn get_browser_cell() -> &'static SharedBrowser {
    BROWSER.get_or_init(|| Arc::new(Mutex::new(None)))
}

async fn ensure_browser() -> Result<Arc<Page>, String> {
    let cell = get_browser_cell();
    let mut guard = cell.lock().await;

    if let Some((_, ref page)) = *guard {
        if page.get_title().await.is_ok() {
            return Ok(page.clone());
        }
        tracing::warn!("Browser page unresponsive, re-launching");
    }

    let mut cfg = BrowserConfig::builder()
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .arg("--disable-gpu")
        .arg("--disable-extensions")
        .arg("--disable-background-networking")
        .window_size(1280, 720);

    if std::env::var("VELO_BROWSER_DEBUG").is_err() {
        cfg = cfg.arg("--headless=new");
    }

    let config = cfg.build().map_err(|e| e.to_string())?;
    let (browser, mut handler) = Browser::launch(config).await.map_err(|e| e.to_string())?;

    tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = browser
        .new_page("about:blank")
        .await
        .map_err(|e| e.to_string())?;
    let page = Arc::new(page);
    *guard = Some((browser, page.clone()));
    Ok(page)
}

async fn wait_for_element(page: &Page, selector: &str, timeout: Duration) -> Result<(), String> {
    let start = tokio::time::Instant::now();
    loop {
        if start.elapsed() > timeout {
            return Err(format!("Element '{selector}' not found after {timeout:?}"));
        }
        if page.find_element(selector).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NavigateUrlArgs {
    pub url: String,
    pub wait_seconds: Option<u64>,
}

impl ToolInputT for NavigateUrlArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"url":{"type":"string","description":"Full URL (https://...) to navigate the browser to. Must be called before other browser tools."},"wait_seconds":{"type":"integer","description":"Extra seconds to wait after the page loads. Useful for JavaScript-heavy SPAs that need time to render. Default: 0."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ClickElementArgs {
    pub selector: String,
    pub timeout_secs: Option<u64>,
}

impl ToolInputT for ClickElementArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"selector":{"type":"string","description":"CSS selector to click, e.g. 'button#submit', '.nav-link', 'a[href=\"/login\"]'."},"timeout_secs":{"type":"integer","description":"Maximum seconds to wait for the element to appear before failing. Default: 10."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct JsEvalArgs {
    pub script: String,
}

impl ToolInputT for JsEvalArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"script":{"type":"string","description":"JavaScript code to execute in the page context. Can access DOM (document, window). Return value is JSON-serialized. Examples: 'document.title', 'document.querySelector(\"h1\").textContent'."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FormFillArgs {
    pub selector: String,
    pub value: String,
}

impl ToolInputT for FormFillArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"selector":{"type":"string","description":"CSS selector of the input field to fill, e.g. 'input[name=\"email\"]', '#search-box'."},"value":{"type":"string","description":"Text value to type into the input field. Overwrites existing content."}}}"#
    }
}

#[tool(name = "navigate_url", description = "Navigate the managed browser to a URL. Launches the browser on first use (headless by default; set VELO_BROWSER_DEBUG=1 for visible). Returns final URL and page title. MUST be called before any other browser tool to establish a page context. Waits for the page to load, with optional extra wait for JS-rendered SPAs. BEST FOR: loading a web page before interacting with it via click_element, scrape_page, or fill_form_field. Use http_request to fetch raw API responses without a browser.", input = NavigateUrlArgs)]
#[derive(Default, Clone)]
pub struct NavigateUrlTool;

#[tool(name = "click_element", description = "Click an element on the page identified by CSS selector. Waits up to timeout_secs for the element to appear. BEST FOR: buttons, links, checkboxes. Use fill_form_field first for text inputs. Use evaluate_javascript for complex interactions.", input = ClickElementArgs)]
#[derive(Default, Clone)]
pub struct ClickElementTool;

#[tool(name = "scrape_page", description = "Extract visible text content from the current page (or an element matching CSS selector). Returns up to 8000 characters. BEST FOR: reading article text, extracting data from tables, getting page content after navigation. Use browser_screenshot to capture the visual layout instead.", input = ScrapePageArgs)]
#[derive(Default, Clone)]
pub struct ScrapePageTool;

#[tool(name = "browser_screenshot", description = "Full-page PNG screenshot of the current browser tab, returned as a base64 data URL (data:image/png;base64,...). BEST FOR: visual inspection of rendered pages, verifying layouts. Use capture_screen to capture the entire monitor (including non-browser apps). Use scrape_page to extract text content.", input = ScrapePageArgs)]
#[derive(Default, Clone)]
pub struct BrowserScreenshotTool;

#[tool(name = "evaluate_javascript", description = "Execute arbitrary JavaScript in the current page context. Returns the result as JSON. Accesses full DOM (document, window). POWERFUL but DANGEROUS: avoid mutating page state unless needed. BEST FOR: extracting data not available through scrape_page, triggering custom page behavior, reading dynamic content. Safer alternatives: use scrape_page for text, click_element for clicks.", input = JsEvalArgs)]
#[derive(Default, Clone)]
pub struct EvaluateJavaScriptTool;

#[tool(name = "fill_form_field", description = "Type text into a form input field identified by CSS selector. Overwrites existing content. BEST FOR: filling search boxes, login forms, textareas. Use after navigate_url and before click_element (to submit). For dropdowns/checkboxes, use click_element instead.", input = FormFillArgs)]
#[derive(Default, Clone)]
pub struct FillFormFieldTool;

#[async_trait]
impl ToolRuntime for NavigateUrlTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: NavigateUrlArgs = serde_json::from_value(args)?;
        let page = ensure_browser().await.map_err(exec_err)?;

        page.goto(&a.url)
            .await
            .map_err(|e| exec_err(format!("Navigation failed: {e}")))?;

        tokio::time::sleep(Duration::from_millis(800)).await;

        if let Some(secs) = a.wait_seconds {
            tokio::time::sleep(Duration::from_secs(secs)).await;
        }

        let title = page.get_title().await.ok().flatten().unwrap_or_default();
        let current_url = page.url().await.unwrap_or_default().unwrap_or_default();

        Ok(ToolOutput::ok(format!(
            "Navigated to: {}\nFinal URL: {}\nTitle: {}",
            a.url, current_url, title
        ))
        .into())
    }
}

#[async_trait]
impl ToolRuntime for ClickElementTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: ClickElementArgs = serde_json::from_value(args)?;
        let page = ensure_browser().await.map_err(exec_err)?;
        let timeout = Duration::from_secs(a.timeout_secs.unwrap_or(10));

        wait_for_element(&page, &a.selector, timeout)
            .await
            .map_err(exec_err)?;

        page.find_element(&a.selector)
            .await
            .map_err(|e| exec_err(format!("Element not found: {e}")))?
            .click()
            .await
            .map_err(|e| exec_err(format!("Click failed: {e}")))?;

        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(ToolOutput::ok(format!("Clicked: {}", a.selector)).into())
    }
}

#[async_trait]
impl ToolRuntime for ScrapePageTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: ScrapePageArgs = serde_json::from_value(args)?;
        let page = ensure_browser().await.map_err(exec_err)?;
        let sel = a.selector.unwrap_or_else(|| "body".into());

        let text = page
            .find_element(&sel)
            .await
            .map_err(|e| exec_err(format!("Element '{sel}' not found: {e}")))?
            .inner_text()
            .await
            .map_err(|e| exec_err(e.to_string()))?
            .unwrap_or_default();

        let truncated = if text.len() > 8000 {
            format!("{}\n\n[...truncated]", &text[..8000])
        } else {
            text
        };

        Ok(ToolOutput::ok(truncated).into())
    }
}

#[async_trait]
impl ToolRuntime for BrowserScreenshotTool {
    async fn execute(&self, _args: Value) -> Result<Value, ToolCallError> {
        let page = ensure_browser().await.map_err(exec_err)?;

        let bytes = page
            .screenshot(
                chromiumoxide::page::ScreenshotParams::builder()
                    .full_page(true)
                    .build(),
            )
            .await
            .map_err(|e| exec_err(e.to_string()))?;

        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(ToolOutput::ok(format!("data:image/png;base64,{b64}")).into())
    }
}

#[async_trait]
impl ToolRuntime for EvaluateJavaScriptTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: JsEvalArgs = serde_json::from_value(args)?;
        let page = ensure_browser().await.map_err(exec_err)?;

        let result = page
            .evaluate(a.script.as_str())
            .await
            .map_err(|e| exec_err(format!("JS eval failed: {e}")))?;

        Ok(ToolOutput::ok(format!("{result:?}")).into())
    }
}

#[async_trait]
impl ToolRuntime for FillFormFieldTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: FormFillArgs = serde_json::from_value(args)?;
        let page = ensure_browser().await.map_err(exec_err)?;

        let escaped_sel = a.selector.replace('\\', "\\\\").replace('\'', "\\'");
        let val_json = serde_json::Value::String(a.value.clone());
        let script = format!(
            r#"(() => {{
                const el = document.querySelector('{}');
                if (!el) throw new Error('Element not found');
                el.value = '';
                el.value = {};
                el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                return el.value;
            }})()"#,
            escaped_sel, val_json
        );

        page.evaluate(script.as_str())
            .await
            .map_err(|e| exec_err(format!("Form fill failed: {e}")))?;

        Ok(ToolOutput::ok(format!("Filled '{}' into {}", a.value, a.selector)).into())
    }
}
