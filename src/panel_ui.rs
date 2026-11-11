use crate::{Command, Snapshot, tray_icon::{Appearance, Orientation, Style}};
use slint::{ModelRc, VecModel};
use std::{rc::Rc, sync::mpsc};

slint::slint! {
    import { Button, ComboBox, ScrollView } from "std-widgets.slint";
    export struct PanelRow {
        id: string,
        name: string,
        value: string,
        detail: string,
        source: string,
    }
    export component TrayPanel inherits Window {
        background: transparent;
        in property <[PanelRow]> rows;
        in property <length> panel-x;
        in property <length> panel-y;
        in property <length> panel-width: 320px;
        in property <length> panel-height: 440px;
        in property <int> orientation;
        in property <int> icon-style;
        in property <bool> english;
        out property <length> desired-height: root.settings ? 370px : min(560px, 82px + root.rows.length * 54px + (root.expanded-id != "" ? 90px : 0px));
        in-out property <bool> settings: false;
        in-out property <string> expanded-id;
        callback dismiss();
        callback open-details();
        callback set-language(int);
        callback set-orientation(int);
        callback set-style(int);

        TouchArea { clicked => { root.dismiss(); } }
        Rectangle {
            x: root.panel-x;
            y: root.panel-y;
            width: root.panel-width;
            height: root.panel-height;
            background: #202831;
            border-radius: 10px;
            border-width: 1px;
            border-color: #5a6673;
            // Consume background clicks so only clicks outside this panel dismiss it.
            TouchArea {}
            VerticalLayout {
                padding: 14px;
                spacing: 9px;
                HorizontalLayout {
                    spacing: 8px;
                    Text { text: root.settings ? (root.english ? "Settings" : "Настройки") : "Battery Life"; color: #f2f5f8; font-size: 16px; font-weight: 600; vertical-alignment: center; }
                    Button { text: root.settings ? (root.english ? "Back" : "Назад") : (root.english ? "Settings" : "Настройки"); clicked => { root.settings = !root.settings; } }
                    Button { text: "×"; max-width: 34px; clicked => { root.dismiss(); } }
                }
                if !root.settings: ScrollView {
                    vertical-stretch: 1;
                    VerticalLayout {
                        spacing: 6px;
                        alignment: start;
                        if root.rows.length == 0: Text {
                            text: root.english ? "No devices detected" : "Устройства не обнаружены";
                            color: #bac5ce;
                            wrap: word-wrap;
                        }
                        for row[index] in root.rows: Rectangle {
                            height: content.preferred-height + 16px;
                            border-radius: 6px;
                            background: target.has-hover ? #354452 : #2a3540;
                            VerticalLayout {
                                content := VerticalLayout {
                                    padding: 8px;
                                    spacing: 6px;
                                    HorizontalLayout {
                                        spacing: 14px;
                                        Text { text: row.name; color: #f2f5f8; font-size: 14px; overflow: elide; }
                                        Text { text: row.value; color: #b7d9c9; font-size: 14px; horizontal-alignment: right; }
                                    }
                                    if root.expanded-id == row.id: Text { text: row.detail; color: #bcc8d2; font-size: 12px; wrap: word-wrap; }
                                    if root.expanded-id == row.id: Text { text: row.source; color: #9fafbd; font-size: 11px; wrap: word-wrap; }
                                }
                            }
                            target := TouchArea {
                                clicked => { root.expanded-id = root.expanded-id == row.id ? "" : row.id; }
                            }
                        }
                    }
                }
                if root.settings: VerticalLayout {
                    spacing: 12px;
                    alignment: start;
                    Text { text: root.english ? "Icon orientation" : "Ориентация значка"; color: #f2f5f8; }
                    ComboBox {
                        model: root.english ? ["Default", "Rotate left", "Rotate right", "Upside down"] : ["Обычная", "Набок влево", "Набок вправо", "Вверх ногами"];
                        current-index: root.orientation;
                        selected(value) => { root.set-orientation(self.current-index); }
                    }
                    Text { text: root.english ? "Icon style" : "Стиль значка"; color: #f2f5f8; }
                    ComboBox {
                        model: root.english ? ["Regular", "Monochrome"] : ["Обычный", "Монохром"];
                        current-index: root.icon-style;
                        selected(value) => { root.set-style(self.current-index); }
                    }
                    Text { text: root.english ? "Language" : "Язык"; color: #f2f5f8; }
                    ComboBox {
                        model: ["Русский", "English"];
                        current-index: root.english ? 1 : 0;
                        selected(value) => { root.set-language(self.current-index); }
                    }
                    Button { text: root.english ? "Open window" : "Открыть окно"; clicked => { root.open-details(); root.dismiss(); } }
                    Rectangle { vertical-stretch: 1; }
                }
            }
        }
    }
}

pub fn create(
    tx: mpsc::Sender<Command>,
    dismiss: mpsc::Sender<()>,
) -> anyhow::Result<TrayPanel> {
    let panel = TrayPanel::new()?;
    panel.on_dismiss(move || { let _ = dismiss.send(()); });
    let sender = tx.clone();
    panel.on_open_details(move || { let _ = sender.send(Command::OpenWindow); });
    let sender = tx.clone();
    panel.on_set_orientation(move |index| {
        if let Some((orientation, _)) = usize::try_from(index).ok().and_then(|i| Orientation::OPTIONS.get(i)) {
            let _ = sender.send(Command::SetOrientation(*orientation));
        }
    });
    let sender = tx.clone();
    panel.on_set_style(move |index| {
        if let Some((style, _)) = usize::try_from(index).ok().and_then(|i| Style::OPTIONS.get(i)) {
            let _ = sender.send(Command::SetStyle(*style));
        }
    });
    panel.on_set_language(move |index| {
        let language = match index {
            0 => crate::i18n::Language::Russian,
            1 => crate::i18n::Language::English,
            _ => return,
        };
        let _ = tx.send(Command::SetLanguage(language));
    });
    Ok(panel)
}

pub fn update(panel: &TrayPanel, snapshot: &Snapshot, appearance: Appearance) {
    let language = appearance.language;
    panel.set_english(language == crate::i18n::Language::English);
    let rows = snapshot.visible_devices().map(|device| {
        let source = match device.reading_time() {
            Some(time) => format!("{} · {time}", device.source),
            None => device.source.clone(),
        };
        PanelRow { id: device.id.clone().into(), name: language.translate(&device.name).into(), value: language.translate(&device.label()).into(), detail: language.translate(&device.detail).into(), source: language.translate(&source).into() }
    }).collect::<Vec<_>>();
    panel.set_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    panel.set_orientation(Orientation::OPTIONS.iter().position(|(v, _)| *v == appearance.orientation).unwrap_or_default() as i32);
    panel.set_icon_style(Style::OPTIONS.iter().position(|(v, _)| *v == appearance.style).unwrap_or_default() as i32);
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::{ComponentHandle, PhysicalSize, LogicalPosition, platform::{Platform, WindowAdapter, PlatformError, WindowEvent, PointerEventButton, software_renderer::{MinimalSoftwareWindow, RepaintBufferType, PremultipliedRgbaColor}}};

    struct TestPlatform(Rc<MinimalSoftwareWindow>);
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> { Ok(self.0.clone()) }
    }

    #[test]
    fn panel_keeps_details_and_settings_inside_one_surface() {
        let surface = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(surface.clone()))).unwrap();
        surface.set_size(PhysicalSize::new(800, 700));
        let (sender, commands) = mpsc::channel();
        let (dismiss, dismissed) = mpsc::channel();
        let panel = create(sender, dismiss).unwrap();
        panel.set_panel_x(20.0);
        panel.set_panel_y(20.0);
        panel.set_panel_width(320.0);
        panel.set_panel_height(480.0);
        let mut device = crate::model::Device::new("probe", "ZAOPIN Z2 Max", "");
        device.reading(Some(45), None, Some(false), true, "USB HID · Compx");
        let snapshot = Snapshot { version: "test", checked_at: 0, devices: vec![device], warnings: vec![] };
        update(&panel, &snapshot, Appearance::default());
        panel.show().unwrap();
        let mut pixels = vec![PremultipliedRgbaColor::default(); 800*700];
        let mut render = || {
            slint::platform::update_timers_and_animations();
            surface.draw_if_needed(|renderer| { renderer.render(&mut pixels, 800); });
        };
        render();
        let click = |x, y| {
            let position = LogicalPosition::new(x, y);
            surface.dispatch_event(WindowEvent::PointerMoved { position });
            surface.dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
            surface.dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
        };
        click(100.0, 100.0);
        assert_eq!(panel.get_expanded_id(), "probe");
        render();
        click(270.0, 50.0);
        assert!(panel.get_settings());
        render();
        panel.invoke_set_orientation(1);
        assert!(matches!(commands.try_recv(), Ok(Command::SetOrientation(Orientation::Left))));
        panel.invoke_set_style(0);
        assert!(matches!(commands.try_recv(), Ok(Command::SetStyle(Style::Color))));
        panel.invoke_set_language(1);
        assert!(matches!(commands.try_recv(), Ok(Command::SetLanguage(crate::i18n::Language::English))));
        let english = Appearance { language: crate::i18n::Language::English, ..Appearance::default() };
        update(&panel, &snapshot, english);
        assert!(panel.get_english());
        click(500.0, 100.0);
        assert!(dismissed.try_recv().is_ok());
    }
}

