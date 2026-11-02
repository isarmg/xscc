use serde::de::DeserializeOwned;
use serde_json::Value;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_namespace = ["globalThis", "kvmBrowser"])]
extern "C" {
    #[wasm_bindgen(catch, js_name = request)]
    async fn browser_request(
        path: &str,
        method: &str,
        body: Option<String>,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = setCsrf)]
    fn browser_csrf(value: &str);
    #[wasm_bindgen(js_name = scopedPath)]
    pub fn url(path: &str) -> String;
    #[wasm_bindgen(js_name = instanceId)]
    pub fn instance_id() -> String;
}

#[derive(Clone, Debug)]
pub struct Error {
    pub status: u16,
    pub message: String,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.status == 0 {
            f.write_str(&self.message)
        } else {
            write!(f, "{}（HTTP {}）", self.message, self.status)
        }
    }
}

pub fn csrf(value: &str) {
    browser_csrf(value);
}

pub async fn request<T: DeserializeOwned>(
    path: &str,
    method: &str,
    value: Option<Value>,
) -> Result<T, Error> {
    let raw = browser_request(path, method, value.map(|value| value.to_string()))
        .await
        .map_err(|error| Error {
            status: js_sys::Reflect::get(&error, &JsValue::from_str("status"))
                .ok()
                .and_then(|value| value.as_f64())
                .unwrap_or_default() as u16,
            message: js_sys::Reflect::get(&error, &JsValue::from_str("message"))
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_else(|| "请求失败，请检查连接。".into()),
        })?;
    let text = raw.as_string().ok_or_else(|| Error {
        status: 0,
        message: "服务端响应不是文本。".into(),
    })?;
    serde_json::from_str(&text).map_err(|error| Error {
        status: 0,
        message: format!("服务端响应格式错误：{error}"),
    })
}

pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, Error> {
    request(path, "GET", None).await
}

pub async fn mutate(path: &str, method: &str, value: Option<Value>) -> Result<(), Error> {
    request::<Value>(path, method, value).await.map(|_| ())
}
