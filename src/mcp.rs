use std::sync::Arc;

use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::grok::GrokClient;

/// MCPツールに渡す検索パラメーターです。
#[derive(Debug, Deserialize, JsonSchema)]
struct SearchParams {
    /// Grokでウェブ検索し、回答してほしい質問です。
    query: String,
}

/// contextXのMCPリクエストを処理します。
#[derive(Clone)]
pub struct ContextXServer {
    grok_client: Arc<GrokClient>,
    #[expect(dead_code, reason = "tool_handlerマクロがこのルーターを参照します")]
    tool_router: ToolRouter<Self>,
}

impl ContextXServer {
    pub fn new(grok_client: Arc<GrokClient>) -> Self {
        Self {
            grok_client,
            tool_router: Self::tool_router(),
        }
    }
}

/// 空のクエリを上流に送らないよう検証します。
fn validate_query(query: &str) -> Result<&str, ErrorData> {
    let query = query.trim();
    if query.is_empty() {
        return Err(ErrorData::invalid_params(
            "`query`には空でない文字列を指定してください",
            None,
        ));
    }

    Ok(query)
}

/// 検索結果をMCPのツール応答へ変換します。
fn into_tool_result(answer: Result<String, String>) -> Result<CallToolResult, ErrorData> {
    match answer {
        Ok(answer) => Ok(CallToolResult::success(vec![ContentBlock::text(answer)])),
        Err(message) => Ok(CallToolResult::error(vec![ContentBlock::text(message)])),
    }
}

#[tool_router]
impl ContextXServer {
    #[tool(description = "【普通検索】Grok 4.3でウェブ検索し、質問に簡潔に回答します。既定の検索手段として使用します")]
    async fn grok_search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let query = validate_query(&params.query)?;

        into_tool_result(self.grok_client.search(query).await)
    }

    #[tool(
        description = "【深度検索】Grok 4.20 Multi-Agent 0309で複数の情報源を照合し、包括的に調査します。利用者が深度検索を明示的に要求した場合に使用します"
    )]
    async fn grok_deep_search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let query = validate_query(&params.query)?;

        into_tool_result(self.grok_client.deep_search(query).await)
    }

    #[tool(
        description = "【究極検索】Grok 4.20 Multi-Agent 0309に最大限の推論を行わせ、徹底的に検証した詳細な調査結果を返します。応答に数分かかるため、利用者が究極検索を明示的に要求した場合にのみ使用します"
    )]
    async fn grok_ultra_search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let query = validate_query(&params.query)?;

        into_tool_result(self.grok_client.ultra_search(query).await)
    }
}

#[tool_handler]
impl ServerHandler for ContextXServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("contextX", env!("CARGO_PKG_VERSION"))
                    .with_title("contextX Grok検索"),
            )
            .with_instructions(
                "検索は3段階です。普通検索（grok_search、Grok 4.3）を既定として使用します。利用者が深度検索を明示的に要求した場合は深度検索（grok_deep_search）を使用します。利用者が究極検索を明示的に要求した場合のみ究極検索（grok_ultra_search）を使用します。究極検索は最大限の推論を行うため応答に数分かかります。",
            )
    }
}
