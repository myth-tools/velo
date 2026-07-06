use autoagents::core::tool::ToolInputT;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct EmptyArgs {}

impl ToolInputT for EmptyArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ScrapePageArgs {
    pub selector: Option<String>,
}

impl ToolInputT for ScrapePageArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"selector":{"type":"string","description":"Optional CSS selector to target a specific page element (e.g. 'article.main', '#content', 'table'). If omitted or null, the entire page body is used. For scrape_page: returns text of the targeted element(s). For browser_screenshot: captures the targeted element visually."}}}"#
    }
}
