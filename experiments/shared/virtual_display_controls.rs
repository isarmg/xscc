use crate::{text_field::TextField, theme};
use gpui::{App, AppContext, Entity, EntityInputHandler, Window, div, prelude::*, px, rgb};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh_rate_units: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    pub id: String,
    pub name: String,
    pub index: Option<usize>,
    pub active: bool,
    pub mode: Mode,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub protocol_version: u32,
    pub driver: String,
    pub status: String,
    pub message: String,
    pub configured_count: u32,
    pub present_count: u32,
    pub active_count: u32,
    pub persistent: bool,
    pub maximum_count: Option<u32>,
    pub displays: Vec<Screen>,
    pub modes: Vec<Mode>,
}

impl State {
    pub fn configurable(&self) -> bool {
        self.protocol_version >= 1
            && matches!(
                self.status.as_str(),
                "ready" | "not_installed" | "disabled" | "reboot_required" | "error"
            )
    }

    pub fn status_label(&self) -> &'static str {
        match self.status.as_str() {
            "ready" => "驱动就绪",
            "not_installed" => "驱动尚未安装",
            "disabled" => "驱动已禁用",
            "external_owned" => "虚拟适配器由其他程序管理",
            "reboot_required" => "需要重启被控端 Windows",
            "error" => "虚拟显示器故障",
            "unsupported" => "当前被控端不支持",
            _ => "状态尚未确认",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Configuration {
    pub count: u32,
    pub mode: Option<Mode>,
    pub persistent: bool,
}

pub struct Editor {
    pub count: Entity<TextField>,
    pub width: Entity<TextField>,
    pub height: Entity<TextField>,
    pub refresh_rate: Entity<TextField>,
    pub persistent: bool,
    pub use_mode: bool,
    pub editing: bool,
    pub error: Option<String>,
}

impl Editor {
    pub fn new(cx: &mut App) -> Self {
        Self {
            count: cx.new(|cx| TextField::new("虚拟屏数量", false, cx)),
            width: cx.new(|cx| TextField::new("宽度", false, cx)),
            height: cx.new(|cx| TextField::new("高度", false, cx)),
            refresh_rate: cx.new(|cx| TextField::new("刷新率（Hz）", false, cx)),
            persistent: false,
            use_mode: false,
            editing: false,
            error: None,
        }
    }

    pub fn begin(&mut self, state: &State, window: &mut Window, cx: &mut App) {
        replace(&self.count, &state.configured_count.to_string(), window, cx);
        self.persistent = state.persistent;
        if let Some(mode) = state
            .displays
            .iter()
            .find(|screen| screen.active)
            .map(|screen| screen.mode)
            .or_else(|| state.modes.first().copied())
        {
            self.set_mode(mode, window, cx);
        }
        self.use_mode = false;
        self.editing = true;
        self.error = None;
    }

    pub fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut App) {
        replace(&self.width, &mode.width.to_string(), window, cx);
        replace(&self.height, &mode.height.to_string(), window, cx);
        replace(
            &self.refresh_rate,
            &format_refresh_rate(mode.refresh_rate_units),
            window,
            cx,
        );
        self.use_mode = true;
    }

    pub fn configuration(&self, cx: &App) -> Result<Configuration, String> {
        if self.count.read(cx).is_composing() {
            return Err("请先完成输入。".into());
        }
        let count = self
            .count
            .read(cx)
            .value()
            .trim()
            .parse::<u32>()
            .map_err(|_| "虚拟屏数量必须是非负整数。")?;
        let mode = if self.use_mode && count > 0 {
            if [&self.width, &self.height, &self.refresh_rate]
                .iter()
                .any(|field| field.read(cx).is_composing())
            {
                return Err("请先完成输入。".into());
            }
            let width = self
                .width
                .read(cx)
                .value()
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or("宽度必须是正整数。")?;
            let height = self
                .height
                .read(cx)
                .value()
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or("高度必须是正整数。")?;
            let refresh_rate_units = parse_refresh_rate(self.refresh_rate.read(cx).value())?;
            Some(Mode {
                width,
                height,
                refresh_rate_units,
            })
        } else {
            None
        };
        Ok(Configuration {
            count,
            mode,
            persistent: self.persistent,
        })
    }

    pub fn fields(&self, palette: theme::Palette) -> gpui::Div {
        div().flex().flex_col().gap_2()
            .child(div().text_size(px(12.)).child("目标虚拟屏数量（0 为全部移除）"))
            .child(self.count.clone())
            .when(self.use_mode, |body| {
                body.child(div().flex().gap_2()
                    .child(div().flex_1().min_w_0().child(self.width.clone()))
                    .child(div().flex_1().min_w_0().child(self.height.clone())))
                    .child(div().text_size(px(12.)).child("显示刷新率（Hz，最多四位小数）"))
                    .child(self.refresh_rate.clone())
            })
            .child(div().text_size(px(12.)).text_color(rgb(palette.muted))
                .child("保留策略作用于本机全部 XSCC 虚拟屏；会话模式会在最后一个远控会话断开后清除。"))
    }
}

fn replace(field: &Entity<TextField>, value: &str, window: &mut Window, cx: &mut App) {
    field.update(cx, |field, cx| {
        let end = field.value().encode_utf16().count();
        field.replace_text_in_range(Some(0..end), value, window, cx);
    });
}

pub fn summary(state: &State, palette: theme::Palette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(state.status_label()),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(palette.muted))
                .child(format!(
                    "已配置 {} · 已识别 {} · 活动 {} · {}",
                    state.configured_count,
                    state.present_count,
                    state.active_count,
                    if state.persistent {
                        "长期保留"
                    } else {
                        "会话模式"
                    }
                )),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(palette.muted))
                .child(
                    state
                        .maximum_count
                        .map(|count| format!("主机允许最多 {count} 个虚拟屏"))
                        .unwrap_or_else(|| {
                            "数量、分辨率及刷新率由主机驱动与 Windows 实际验证。".into()
                        }),
                ),
        )
        .when(
            matches!(state.status.as_str(), "not_installed" | "disabled"),
            |body| {
                body.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.muted))
                        .child("应用配置会在被控端安装或启用随包提供的签名驱动。"),
                )
            },
        )
        .when(!state.message.is_empty(), |body| {
            body.child(div().text_size(px(12.)).child(state.message.clone()))
        })
}

pub fn mode_label(mode: Mode) -> String {
    format!(
        "{}×{} · {} Hz",
        mode.width,
        mode.height,
        format_refresh_rate(mode.refresh_rate_units)
    )
}

pub fn format_refresh_rate(units: u32) -> String {
    let integer = units / 10_000;
    let fraction = units % 10_000;
    if fraction == 0 {
        integer.to_string()
    } else {
        format!("{integer}.{fraction:04}")
            .trim_end_matches('0')
            .to_owned()
    }
}

pub fn parse_refresh_rate(value: &str) -> Result<u32, String> {
    let value = value.trim();
    let (integer, fraction) = value.split_once('.').unwrap_or((value, ""));
    if integer.is_empty()
        || !integer.bytes().all(|c| c.is_ascii_digit())
        || fraction.len() > 4
        || !fraction.bytes().all(|c| c.is_ascii_digit())
    {
        return Err("刷新率必须是正数，最多四位小数。".into());
    }
    let integer = integer.parse::<u32>().map_err(|_| "刷新率超出范围。")?;
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u32>().map_err(|_| "刷新率无效。")? * 10_u32.pow(4 - fraction.len() as u32)
    };
    integer
        .checked_mul(10_000)
        .and_then(|value| value.checked_add(fraction))
        .filter(|value| *value > 0)
        .ok_or_else(|| "刷新率必须大于 0 且未超出范围。".into())
}

#[cfg(test)]
mod tests {
    use super::{format_refresh_rate, parse_refresh_rate};

    #[test]
    fn display_refresh_rate_preserves_ten_thousandth_hz_precision() {
        for (text, units) in [("60", 600_000), ("59.94", 599_400), ("119.9999", 1_199_999)] {
            assert_eq!(parse_refresh_rate(text).unwrap(), units);
            assert_eq!(format_refresh_rate(units), text);
        }
        for invalid in ["0", "NaN", "60.00001", "4294967295", "-1", "1e2"] {
            assert!(parse_refresh_rate(invalid).is_err());
        }
    }
}
