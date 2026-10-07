//! The InnerTube transport: request context, headers and JSON POST helper.

use crate::clients::{self, YtClient, API_URL_YOUTUBE_MUSIC};
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Map, Value};
use std::sync::RwLock;
use std::time::Duration;

/// A reusable, thread-safe InnerTube transport.
///
/// The blocking `reqwest` client is used deliberately: the app runs network
/// work on dedicated worker threads and never on the UI thread, so an async
/// runtime would add complexity without any benefit.
pub struct InnerTube {
    http: reqwest::blocking::Client,
    gl: String,
    hl: String,
    visitor_data: RwLock<Option<String>>,
    cookie: RwLock<Option<String>>,
    /// When true, request/response bodies are logged at debug level.
    pub trace: bool,
}

impl InnerTube {
    /// Builds a transport for the given region/language and optional proxy.
    pub fn new(
        gl: impl Into<String>,
        hl: impl Into<String>,
        proxy: Option<String>,
    ) -> Result<Self> {
        let mut builder = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(15))
            .pool_max_idle_per_host(10)
            .gzip(true)
            .brotli(true)
            .deflate(true);

        if let Some(proxy_url) = proxy.as_deref().filter(|p| !p.is_empty()) {
            let proxy = reqwest::Proxy::all(proxy_url)
                .with_context(|| format!("invalid proxy `{proxy_url}`"))?;
            builder = builder.proxy(proxy);
        }

        let http = builder.build().context("failed to build the HTTP client")?;
        Ok(Self {
            http,
            gl: gl.into(),
            hl: hl.into(),
            visitor_data: RwLock::new(None),
            cookie: RwLock::new(None),
            trace: false,
        })
    }

    pub fn gl(&self) -> &str {
        &self.gl
    }

    pub fn hl(&self) -> &str {
        &self.hl
    }

    pub fn set_locale(&mut self, gl: impl Into<String>, hl: impl Into<String>) {
        self.gl = gl.into();
        self.hl = hl.into();
    }

    pub fn visitor_data(&self) -> Option<String> {
        self.visitor_data.read().ok().and_then(|v| v.clone())
    }

    pub fn set_visitor_data(&self, value: Option<String>) {
        if let Ok(mut guard) = self.visitor_data.write() {
            *guard = value;
        }
    }

    pub fn set_cookie(&self, value: Option<String>) {
        if let Ok(mut guard) = self.cookie.write() {
            *guard = value;
        }
    }

    /// Builds the `context` object for a request made as `client`.
    pub fn context(&self, client: &YtClient) -> Value {
        let mut ctx = Map::new();
        ctx.insert("clientName".into(), json!(client.name));
        ctx.insert("clientVersion".into(), json!(client.version));
        ctx.insert("hl".into(), json!(self.hl));
        ctx.insert("gl".into(), json!(self.gl));
        ctx.insert("userAgent".into(), json!(client.user_agent));
        ctx.insert("originalUrl".into(), json!(clients::ORIGIN_YOUTUBE_MUSIC));

        if let Some(vd) = self.visitor_data() {
            ctx.insert("visitorData".into(), json!(vd));
        }
        if let Some(os) = client.os_name {
            ctx.insert("osName".into(), json!(os));
        }
        if let Some(os) = client.os_version {
            ctx.insert("osVersion".into(), json!(os));
        }
        if let Some(make) = client.device_make {
            ctx.insert("deviceMake".into(), json!(make));
        }
        if let Some(model) = client.device_model {
            ctx.insert("deviceModel".into(), json!(model));
        }
        if let Some(sdk) = client.android_sdk_version {
            ctx.insert("androidSdkVersion".into(), json!(sdk));
        }
        if let Some(build) = client.build_id {
            ctx.insert("buildId".into(), json!(build));
        }
        if let Some(cronet) = client.cronet_version {
            ctx.insert("cronetVersion".into(), json!(cronet));
        }

        json!({
            "context": {
                "client": Value::Object(ctx),
                "user": { "lockedSafetyMode": false },
                "request": { "useSsl": true }
            }
        })
    }

    /// Merges a request body with the freshly built context.
    pub fn body(&self, client: &YtClient, extra: Value) -> Value {
        let mut body = self.context(client);
        if let (Some(base), Some(extra_map)) = (body.as_object_mut(), extra.as_object()) {
            for (key, value) in extra_map {
                base.insert(key.clone(), value.clone());
            }
        }
        body
    }

    /// POSTs `body` to `endpoint` as `client` and returns the decoded JSON.
    ///
    /// Falls back to a key-less request when YouTube rejects the API key,
    /// which keeps the client working across key rotations.
    pub fn post(&self, endpoint: &str, client: &YtClient, body: &Value) -> Result<Value> {
        if self.trace {
            log::debug!("→ {endpoint} as {}", client.friendly_name);
        }
        match self.post_inner(endpoint, client, body, true) {
            Ok(value) => Ok(value),
            Err(err) => {
                if clients::api_key(client).is_some() {
                    log::debug!("retrying {endpoint} without api key after: {err}");
                    self.post_inner(endpoint, client, body, false)
                } else {
                    Err(err)
                }
            }
        }
    }

    fn post_inner(
        &self,
        endpoint: &str,
        client: &YtClient,
        body: &Value,
        with_key: bool,
    ) -> Result<Value> {
        let mut url = format!("{API_URL_YOUTUBE_MUSIC}{endpoint}?prettyPrint=false");
        if with_key {
            if let Some(key) = clients::api_key(client) {
                url = format!("{API_URL_YOUTUBE_MUSIC}{endpoint}?key={key}&prettyPrint=false");
            }
        }

        let mut request = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-Goog-Api-Format-Version", "1")
            .header("X-YouTube-Client-Name", client.id)
            .header("X-YouTube-Client-Version", client.version)
            .header("X-Origin", clients::ORIGIN_YOUTUBE_MUSIC)
            .header("Referer", "https://music.youtube.com/")
            .header(
                "Accept-Language",
                format!("{},{},en;q=0.8", self.hl, self.gl),
            )
            .header("User-Agent", client.user_agent);

        if let Some(vd) = self.visitor_data() {
            request = request.header("X-Goog-Visitor-Id", vd);
        }
        if let Some(cookie) = self.cookie.read().ok().and_then(|c| c.clone()) {
            request = request.header("Cookie", cookie);
        }

        let response = request
            .json(body)
            .send()
            .with_context(|| format!("request to {endpoint} failed"))?;

        let status = response.status();
        let text = response
            .text()
            .with_context(|| format!("failed to read the {endpoint} response body"))?;

        if !status.is_success() {
            return Err(anyhow!(
                "InnerTube `{endpoint}` as {} returned HTTP {status}: {}",
                client.friendly_name,
                crate::util::snippet(&text, 280)
            ));
        }

        let value: Value = serde_json::from_str(&text)
            .with_context(|| format!("`{endpoint}` returned invalid JSON"))?;

        // InnerTube rotates the visitor id on most responses; keep it fresh.
        if let Some(vd) = extract_visitor_data(&value) {
            self.set_visitor_data(Some(vd));
        }
        Ok(value)
    }

    /// Fetches a plain-text resource (used for `base.js` and similar assets).
    pub fn get(&self, url: &str) -> Result<String> {
        let response = self
            .http
            .get(url)
            .header("User-Agent", clients::WEB_REMIX.user_agent)
            .send()
            .with_context(|| format!("GET {url} failed"))?;
        let status = response.status();
        let text = response
            .text()
            .with_context(|| format!("failed to read the body of {url}"))?;
        if !status.is_success() {
            return Err(anyhow!("GET {url} returned HTTP {status}"));
        }
        Ok(text)
    }

    /// Fetches a fresh `visitorData` value and stores it.
    pub fn refresh_visitor_data(&self) -> Result<String> {
        let body = self.body(&clients::WEB_REMIX, json!({}));
        let value = self.post("visitor_id", &clients::WEB_REMIX, &body)?;
        let vd = value
            .get("responseContext")
            .and_then(|c| c.get("visitorData"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| extract_visitor_data(&value))
            .ok_or_else(|| anyhow!("visitor_id response contained no visitorData"))?;
        self.set_visitor_data(Some(vd.clone()));
        Ok(vd)
    }

    /// Ensures a visitor id is present, fetching one on first use.
    pub fn ensure_visitor_data(&self) -> Result<String> {
        if let Some(vd) = self.visitor_data() {
            return Ok(vd);
        }
        self.refresh_visitor_data()
    }
}

/// Pulls `responseContext.visitorData` out of any InnerTube response.
pub fn extract_visitor_data(value: &Value) -> Option<String> {
    value
        .get("responseContext")?
        .get("visitorData")?
        .as_str()
        .map(str::to_string)
}
