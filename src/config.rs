use std::{env, error::Error, fmt, str::FromStr, sync::Arc};

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:3000";
const DEFAULT_ALLOWED_HOSTS: &str = "localhost,127.0.0.1,::1";

/// 上流APIのリクエスト・レスポンス形式です。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApiFormat {
    /// OpenAI Responses API互換（`input` / `output`）。
    #[default]
    Responses,
    /// OpenAI Chat Completions API互換（`messages` / `choices`）。
    Chat,
}

impl FromStr for ApiFormat {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "responses" | "response" => Ok(Self::Responses),
            "chat" | "chat_completions" | "chat-completions" => Ok(Self::Chat),
            _ => Err(ConfigError::InvalidApiFormat(value.trim().to_owned())),
        }
    }
}

/// 各検索モードの上流API設定です。
pub struct UpstreamConfig {
    pub api_key: Arc<str>,
    pub url: Arc<str>,
    pub format: ApiFormat,
}

/// アプリケーションの起動設定です。
pub struct AppConfig {
    pub standard: UpstreamConfig,
    pub deep: UpstreamConfig,
    pub bind_addr: String,
    pub allowed_hosts: Vec<String>,
    /// 設定した場合のみ、MCPエンドポイントでBearerトークン認証を要求します。
    pub auth_token: Option<Arc<str>>,
}

impl AppConfig {
    /// 環境変数および `.env` から設定を読み込みます。
    pub fn load() -> Result<Self, ConfigError> {
        dotenvy::dotenv().ok();

        let standard = UpstreamConfig {
            api_key: required("GROK_API_KEY")?,
            url: required("GROK_UPSTREAM_URL")?,
            format: api_format("GROK_API_FORMAT")?,
        };
        let deep = UpstreamConfig {
            api_key: required("GROK_DEEP_API_KEY")?,
            url: required("GROK_DEEP_UPSTREAM_URL")?,
            format: api_format("GROK_DEEP_API_FORMAT")?,
        };
        let bind_addr = optional("BIND_ADDR").unwrap_or_else(|| DEFAULT_BIND_ADDR.to_owned());
        let allowed_hosts = parse_allowed_hosts(
            &optional("MCP_ALLOWED_HOSTS").unwrap_or_else(|| DEFAULT_ALLOWED_HOSTS.to_owned()),
        );
        let auth_token = optional("MCP_AUTH_TOKEN").map(Arc::from);

        Ok(Self {
            standard,
            deep,
            bind_addr,
            allowed_hosts,
            auth_token,
        })
    }
}

/// 空文字や空白のみの値は未設定として扱います。
fn optional(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// 必須の環境変数を読み込みます。
fn required(name: &'static str) -> Result<Arc<str>, ConfigError> {
    optional(name)
        .map(Arc::from)
        .ok_or(ConfigError::MissingVariable(name))
}

/// 未設定の場合はResponses形式を既定として採用します。
fn api_format(name: &str) -> Result<ApiFormat, ConfigError> {
    match optional(name) {
        Some(value) => value.parse(),
        None => Ok(ApiFormat::default()),
    }
}

/// カンマ区切りの許可ホスト一覧を解析します。
fn parse_allowed_hosts(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_owned)
        .collect()
}

/// 設定の読み込みに失敗した場合のエラーです。
#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    MissingVariable(&'static str),
    InvalidApiFormat(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingVariable(name) => {
                write!(formatter, "必須の環境変数 `{name}` が設定されていません")
            }
            Self::InvalidApiFormat(value) => write!(
                formatter,
                "API形式 `{value}` は不正です。`responses` または `chat` を指定してください"
            ),
        }
    }
}

impl Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::{ApiFormat, ConfigError, DEFAULT_ALLOWED_HOSTS, parse_allowed_hosts};

    #[test]
    fn parses_api_format_aliases() {
        assert_eq!("responses".parse(), Ok(ApiFormat::Responses));
        assert_eq!("Response".parse(), Ok(ApiFormat::Responses));
        assert_eq!("chat".parse(), Ok(ApiFormat::Chat));
        assert_eq!(" CHAT-COMPLETIONS ".parse(), Ok(ApiFormat::Chat));
    }

    #[test]
    fn rejects_unknown_api_format() {
        assert_eq!(
            "grpc".parse::<ApiFormat>(),
            Err(ConfigError::InvalidApiFormat("grpc".to_owned()))
        );
    }

    #[test]
    fn defaults_to_responses_format() {
        assert_eq!(ApiFormat::default(), ApiFormat::Responses);
    }

    #[test]
    fn parses_default_allowed_hosts() {
        assert_eq!(
            parse_allowed_hosts(DEFAULT_ALLOWED_HOSTS),
            ["localhost", "127.0.0.1", "::1"]
        );
    }

    #[test]
    fn trims_and_skips_empty_allowed_hosts() {
        assert_eq!(
            parse_allowed_hosts(" example.hf.space , ,localhost, "),
            ["example.hf.space", "localhost"]
        );
    }
}
