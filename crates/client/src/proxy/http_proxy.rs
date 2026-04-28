use anyhow::{Context, Result};
use base64::Engine;
use httparse::{EMPTY_HEADER, Response};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufStream},
    net::TcpStream,
};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use tokio_native_tls::{native_tls, TlsConnector};
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
use tokio_rustls::TlsConnector;
use url::Url;

use super::AsyncReadWrite;

/// HTTP代理类型枚举
pub(super) enum HttpProxyType<'t> {
    /// HTTP代理（可选身份验证）
    HTTP(Option<HttpProxyAuthorization<'t>>),
    /// HTTPS代理（可选身份验证）
    HTTPS(Option<HttpProxyAuthorization<'t>>),
}

/// HTTP代理身份验证信息
pub(super) struct HttpProxyAuthorization<'t> {
    username: &'t str,
    password: &'t str,
}

/// 解析代理URL，生成对应的HTTP代理类型
pub(super) fn parse_http_proxy<'t>(scheme: &str, proxy: &'t Url) -> HttpProxyType<'t> {
    let auth = proxy.password().map(|password| HttpProxyAuthorization {
        username: proxy.username(),
        password,
    });
    if scheme.starts_with("https") {
        HttpProxyType::HTTPS(auth)
    } else {
        HttpProxyType::HTTP(auth)
    }
}

/// 通过HTTP/HTTPS代理建立目标连接流
pub(crate) async fn connect_http_proxy_stream(
    stream: TcpStream,
    http_proxy: HttpProxyType<'_>,
    rpc_host: (&str, u16),
    proxy_domain: &str,
) -> Result<Box<dyn AsyncReadWrite>> {
    match http_proxy {
        HttpProxyType::HTTP(auth) => http_connect(stream, rpc_host, auth).await,
        HttpProxyType::HTTPS(auth) => https_connect(stream, rpc_host, auth, proxy_domain).await,
    }
    .context("连接HTTP/HTTPS代理失败")
}

/// 建立纯HTTP代理连接
async fn http_connect<T>(
    stream: T,
    target: (&str, u16),
    auth: Option<HttpProxyAuthorization<'_>>,
) -> Result<Box<dyn AsyncReadWrite>>
where
    T: AsyncReadWrite,
{
    let mut stream = BufStream::new(stream);
    let request = make_request(target, auth);
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;
    check_response(&mut stream).await?;
    Ok(Box::new(stream))
}

/// Windows/macOS 平台：建立TLS加密的HTTPS代理连接
#[cfg(any(target_os = "windows", target_os = "macos"))]
async fn https_connect<T>(
    stream: T,
    target: (&str, u16),
    auth: Option<HttpProxyAuthorization<'_>>,
    proxy_domain: &str,
) -> Result<Box<dyn AsyncReadWrite>>
where
    T: AsyncReadWrite,
{
    let tls_connector = TlsConnector::from(native_tls::TlsConnector::new()?);
    let stream = tls_connector.connect(proxy_domain, stream).await?;
    http_connect(stream, target, auth).await
}

/// Linux 平台：建立TLS加密的HTTPS代理连接
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
async fn https_connect<T>(
    stream: T,
    target: (&str, u16),
    auth: Option<HttpProxyAuthorization<'_>>,
    proxy_domain: &str,
) -> Result<Box<dyn AsyncReadWrite>>
where
    T: AsyncReadWrite,
{
    let proxy_domain = rustls_pki_types::ServerName::try_from(proxy_domain)
        .context("域名解析失败")?
        .to_owned();
    let tls_connector = TlsConnector::from(std::sync::Arc::new(http_client_tls::tls_config()));
    let stream = tls_connector.connect(proxy_domain, stream).await?;
    http_connect(stream, target, auth).await
}

/// 构造代理CONNECT请求报文（支持基础认证）
fn make_request(target: (&str, u16), auth: Option<HttpProxyAuthorization<'_>>) -> String {
    let (host, port) = target;
    let mut request = format!(
        "CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\nProxy-Connection: Keep-Alive\r\n"
    );
    if let Some(HttpProxyAuthorization { username, password }) = auth {
        let auth =
            base64::prelude::BASE64_STANDARD.encode(format!("{username}:{password}").as_bytes());
        let auth = format!("Proxy-Authorization: Basic {auth}\r\n");
        request.push_str(&auth);
    }
    request.push_str("\r\n");
    request
}

/// 校验代理服务器响应状态码
async fn check_response<T>(stream: &mut BufStream<T>) -> Result<()>
where
    T: AsyncReadWrite,
{
    let response = recv_response(stream).await?;
    let mut dummy_headers = [EMPTY_HEADER; MAX_RESPONSE_HEADERS];
    let mut parser = Response::new(&mut dummy_headers);
    parser.parse(response.as_bytes())?;

    match parser.code {
        Some(code) => {
            if code == 200 {
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "代理连接失败，HTTP状态码：{code}"
                ))
            }
        }
        None => Err(anyhow::anyhow!(
            "代理连接失败，未返回HTTP状态码：{}",
            parser.reason.unwrap_or("未知原因")
        )),
    }
}

/// 响应头最大长度
const MAX_RESPONSE_HEADER_LENGTH: usize = 4096;
/// 最大响应头数量
const MAX_RESPONSE_HEADERS: usize = 16;

/// 接收并读取代理服务器完整响应头
async fn recv_response<T>(stream: &mut BufStream<T>) -> Result<String>
where
    T: AsyncReadWrite,
{
    let mut response = String::new();
    loop {
        if stream.read_line(&mut response).await? == 0 {
            return Err(anyhow::anyhow!("数据流已结束"));
        }

        if MAX_RESPONSE_HEADER_LENGTH < response.len() {
            return Err(anyhow::anyhow!("超出响应头最大长度限制"));
        }

        if response.ends_with("\r\n\r\n") {
            return Ok(response);
        }
    }
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{HttpProxyAuthorization, HttpProxyType, parse_http_proxy};

    #[test]
    /// 测试无认证的HTTP代理解析
    fn test_parse_http_proxy() {
        let proxy = Url::parse("http://proxy.example.com:1080").unwrap();
        let scheme = proxy.scheme();

        let version = parse_http_proxy(scheme, &proxy);
        assert!(matches!(version, HttpProxyType::HTTP(None)))
    }

    #[test]
    /// 测试带身份认证的HTTP代理解析
    fn test_parse_http_proxy_with_auth() {
        let proxy = Url::parse("http://username:password@proxy.example.com:1080").unwrap();
        let scheme = proxy.scheme();

        let version = parse_http_proxy(scheme, &proxy);
        assert!(matches!(
            version,
            HttpProxyType::HTTP(Some(HttpProxyAuthorization {
                username: "username",
                password: "password"
            }))
        ))
    }
}