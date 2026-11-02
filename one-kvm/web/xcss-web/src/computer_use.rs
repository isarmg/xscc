use crate::ui;
use serde::Deserialize;
use serde_json::json;
use wasm_bindgen::prelude::*;
use xcss_web::{html as h, prelude::*};

#[wasm_bindgen(js_namespace = ["globalThis", "kvmBrowser"])]
extern "C" {
    #[wasm_bindgen(catch, js_name = startComputerUse)]
    async fn start_computer_use(
        prompt: &str,
        continue_conversation: bool,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = stopComputerSocket)]
    fn stop_computer_socket();
}

#[derive(Clone, Deserialize)]
struct Session {
    status: String,
    step: u32,
    last_error: Option<String>,
    final_message: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Message {
    SessionUpdated { session: Session },
    ScreenshotRequested {},
    ScreenshotCaptured {},
    StepStarted { step: u32 },
    ReasoningDelta { delta: String },
    ReasoningCompleted { failed: bool },
    ActionsExecuted {},
    Error { message: String },
}

pub fn panel() -> impl IntoView {
    let prompt = RwSignal::new(String::new());
    let consent = RwSignal::new(false);
    let continue_conversation = RwSignal::new(false);
    let reasoning = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let (session, load_error) = ui::poll::<Session>("/api/computer-use/session", 2500);
    ui::listen("kvm-computer-use", move |event| {
        let Some(text) = js_sys::Reflect::get(event.as_ref(), &JsValue::from_str("detail"))
            .ok()
            .and_then(|value| value.as_string())
        else {
            return;
        };
        match serde_json::from_str::<Message>(&text) {
            Ok(Message::SessionUpdated { session: value }) => session.set(Some(value)),
            Ok(Message::StepStarted { step }) => message.set(format!("执行步骤 {step}")),
            Ok(Message::ReasoningDelta { delta }) => reasoning.update(|value| {
                if value.len() + delta.len() <= 24_000 {
                    value.push_str(&delta);
                }
            }),
            Ok(Message::ReasoningCompleted { failed }) => {
                if failed {
                    error.set("推理步骤失败，请检查模型配置。".into());
                }
            }
            Ok(Message::Error { message }) => error.set(message),
            Ok(
                Message::ScreenshotRequested {}
                | Message::ScreenshotCaptured {}
                | Message::ActionsExecuted {},
            ) => {}
            Err(value) => error.set(format!("Computer Use 事件格式错误：{value}")),
        }
    });
    on_cleanup(stop_computer_socket);
    h::details().class("card").child((
        h::summary().child("Computer Use 自动操作"),
        h::p().class("hint").child("自动操作会将当前画面发送到设备设置中配置的模型服务，并控制当前 KVM 的键鼠。请确认画面和任务内容适合发送后启动；撤权或离开控制台会关闭该浏览器的截图通道。"),
        ui::error(error), ui::error(load_error),
        h::p().class("hint").child(move || session.get().map(|value| format!("{} · 第 {} 步 · {}{}", value.status, value.step, value.final_message.unwrap_or_default(), value.last_error.unwrap_or_default())).unwrap_or_default()),
        h::textarea().attr("aria-label", "自动操作任务").placeholder("描述希望在被控计算机上完成的任务").prop("value", move || prompt.get()).on(xcss_web::ev::input, move |event| prompt.set(event_target_value(&event))),
        h::label().child((h::input().r#type("checkbox").on(xcss_web::ev::change, move |event| consent.set(event_target_checked(&event))), "我确认允许发送当前画面并执行上述任务")),
        h::label().child((h::input().r#type("checkbox").on(xcss_web::ev::change, move |event| continue_conversation.set(event_target_checked(&event))), "继续当前对话")),
        h::div().class("actions").child((
            h::button().class("primary").disabled(move || busy.get() || !consent.get() || prompt.with(String::is_empty)).on(xcss_web::ev::click, move |_| {
                let prompt = prompt.get_untracked(); let continuing = continue_conversation.get_untracked(); busy.set(true); reasoning.set(String::new()); error.set(String::new());
                xcss_web::task::spawn_local(async move { if let Err(value) = start_computer_use(&prompt, continuing).await { let _ = error.try_set(ui::js_error(value)); } let _ = busy.try_set(false); });
            }).child("启动自动操作"),
            ui::action("立即停止", "/api/computer-use/session/stop", "POST", Some(json!({})), error),
        )),
        h::p().class("hint").child(move || message.get()),
        h::pre().class("hint").child(move || reasoning.get()),
        h::p().class("hint").child("模型服务地址、模型名称和密钥由设备设置中的 Computer Use 表单维护；浏览器不会收到保存的 API 密钥。"),
    ))
}
