use std::{error::Error, sync::Arc, time::Duration};

use axum::{
    Router,
    extract::{Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    middleware::{self, Next},
    response::Response,
    routing::get,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use subtle::ConstantTimeEq;

use crate::{config::AppConfig, grok::GrokClient, mcp::ContextXServer};

const BEARER_PREFIX: &str = "Bearer ";
/// 上流の推論待ちで通信が無音になり、ゲートウェイに切断されるのを防ぐping間隔です。
const SSE_KEEP_ALIVE: Duration = Duration::from_secs(15);

/// Streamable HTTP形式のMCPサーバーを起動します。
pub async fn run(config: AppConfig) -> Result<(), Box<dyn Error>> {
    let AppConfig {
        standard,
        deep,
        bind_addr,
        allowed_hosts,
        auth_token,
    } = config;

    let grok_client = Arc::new(GrokClient::new(standard, deep)?);
    let service: StreamableHttpService<ContextXServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(ContextXServer::new(Arc::clone(&grok_client))),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default()
                .with_allowed_hosts(allowed_hosts)
                .with_sse_keep_alive(Some(SSE_KEEP_ALIVE)),
        );

    let mut mcp_router = Router::new().nest_service("/mcp", service);
    if let Some(token) = auth_token {
        println!("MCPエンドポイントのBearerトークン認証を有効化しました");
        mcp_router = mcp_router.layer(middleware::from_fn_with_state(token, authorize));
    } else {
        println!("警告: MCP_AUTH_TOKENが未設定のため、MCPエンドポイントは認証なしで公開されます");
    }

    // ヘルスチェックは監視のため認証対象から除外します。
    let app = Router::new().route("/health", get(health)).merge(mcp_router);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;

    println!("contextXを http://{bind_addr}/mcp で起動しました");
    axum::serve(listener, app).await?;

    Ok(())
}

/// `Authorization: Bearer <token>` ヘッダーを検証します。
async fn authorize(
    State(expected): State<Arc<str>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    if is_authorized(header, &expected) {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// タイミング攻撃を避けるため、トークンを定数時間で比較します。
fn is_authorized(header: Option<&str>, expected: &str) -> bool {
    let Some(presented) = header
        .and_then(|value| value.strip_prefix(BEARER_PREFIX))
        .map(str::trim)
    else {
        return false;
    };

    presented.as_bytes().ct_eq(expected.as_bytes()).into()
}

async fn health() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests {
    use super::is_authorized;

    const TOKEN: &str = "secret-token";

    #[test]
    fn accepts_matching_bearer_token() {
        assert!(is_authorized(Some("Bearer secret-token"), TOKEN));
    }

    #[test]
    fn accepts_bearer_token_with_trailing_whitespace() {
        assert!(is_authorized(Some("Bearer secret-token  "), TOKEN));
    }

    #[test]
    fn rejects_missing_header() {
        assert!(!is_authorized(None, TOKEN));
    }

    #[test]
    fn rejects_wrong_token() {
        assert!(!is_authorized(Some("Bearer wrong-token"), TOKEN));
    }

    #[test]
    fn rejects_token_without_bearer_prefix() {
        assert!(!is_authorized(Some("secret-token"), TOKEN));
    }

    #[test]
    fn rejects_case_mismatched_scheme() {
        assert!(!is_authorized(Some("bearer secret-token"), TOKEN));
    }

    #[test]
    fn rejects_prefix_of_expected_token() {
        assert!(!is_authorized(Some("Bearer secret"), TOKEN));
    }
}
