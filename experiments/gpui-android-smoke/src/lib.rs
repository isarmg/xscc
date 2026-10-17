#![cfg(target_os = "android")]

#[path = "../../gpui-validation/src/text_field.rs"]
mod text_field;
#[path = "../../gpui-validation/src/theme.rs"]
mod theme;

use gpui::{Context, Entity, Window, WindowOptions, div, prelude::*, px, rgb};

struct Smoke {
    address: Entity<text_field::TextField>,
    password: Entity<text_field::TextField>,
    taps: usize,
    wakes: usize,
    font: gpui::SharedString,
}
impl Render for Smoke {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let insets = window.insets().effective();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .px_6()
            .pt(insets.top + px(24.))
            .pb(insets.bottom + px(24.))
            .bg(rgb(0x101622))
            .text_color(rgb(0xf5f7ff))
            .font_family(self.font.clone())
            .child(div().text_2xl().child("GPUI · Android ARM64"))
            .child("原生界面 · 触摸滚动測試")
            .child("Offline IME test · device keyboard verification pending")
            .child(self.address.clone())
            .child(self.password.clone())
            .child(
                div()
                    .id("tap-counter")
                    .px_4()
                    .py_3()
                    .rounded_lg()
                    .bg(rgb(0x246bfd))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.taps += 1;
                        cx.notify();
                    }))
                    .child(format!("Tap / 点击: {}", self.taps)),
            )
            .child(
                div()
                    .id("async-wake")
                    .px_4()
                    .py_3()
                    .rounded_lg()
                    .bg(rgb(0x294359))
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(300))
                                .await;
                            if let Err(error) = this.update(cx, |this, cx| {
                                this.wakes += 1;
                                cx.notify();
                            }) {
                                log::debug!("Async smoke window closed: {error}");
                            }
                        })
                        .detach();
                    }))
                    .child(format!("Async wake / 异步唤醒: {}", self.wakes)),
            )
            .child(
                div()
                    .id("scroll-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children((1..=60).map(|index| {
                        div()
                            .p_3()
                            .border_b_1()
                            .border_color(rgb(0x294359))
                            .child(format!("Row {index:02} · 中文滚动列表"))
                    })),
            )
    }
}

#[unsafe(no_mangle)]
fn android_main(app: gpui::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("XSCC-GPUI")
            .with_max_level(log::LevelFilter::Info),
    );
    let application = match gpui::android(app) {
        Ok(application) => application,
        Err(error) => {
            log::error!("Cannot initialize Android GPUI: {error:#}");
            return;
        }
    };
    application.run(|cx| {
        text_field::init(cx);
        let names = cx.text_system().all_font_names();
        let font = names
            .iter()
            .find(|name| name.as_str() == "Roboto")
            .or_else(|| names.first())
            .cloned()
            .unwrap_or_else(|| "sans-serif".into());
        if let Err(error) = cx.open_window(WindowOptions::default(), move |_, cx| {
            let address =
                cx.new(|cx| text_field::TextField::new("地址 / Chinese + emoji test", false, cx));
            let password =
                cx.new(|cx| text_field::TextField::new("密码 / secure keyboard test", true, cx));
            cx.new(|_| Smoke {
                address,
                password,
                taps: 0,
                wakes: 0,
                font: font.into(),
            })
        }) {
            log::error!("Cannot open Android GPUI window: {error:#}");
            cx.quit();
        }
    });
}
