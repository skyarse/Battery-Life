use crate::{Command, Snapshot};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{rc::Rc, sync::{Arc, Mutex, mpsc}};

slint::slint! {
    import { Button, ScrollView } from "std-widgets.slint";
    export struct DeviceRow {
        name: string,
        value: string,
        detail: string,
        source: string,
        percent: float,
        has-percent: bool,
        stale: bool,
    }
    export component DetailsWindow inherits Window {
        title: "Battery Life";
        preferred-width: 620px;
        preferred-height: 750px;
        min-width: 500px;
        min-height: 560px;
        background: #18212b;
        in property <[DeviceRow]> rows;
        in property <string> status;
        callback refresh();
        VerticalLayout {
            padding: 20px;
            spacing: 12px;
            Text { text: "Battery Life"; color: #f2f5f8; font-size: 26px; font-weight: 700; }
            Text { text: "Заряд беспроводных устройств"; color: #a6b4c3; font-size: 14px; }
            ScrollView {
                vertical-stretch: 1;
                VerticalLayout {
                    spacing: 8px;
                    if root.rows.length == 0: Text {
                        text: "Устройства не обнаружены";
                        color: #a6b4c3;
                    }
                    for row in root.rows: Rectangle {
                        height: 106px;
                        background: #222e3b;
                        border-radius: 8px;
                        VerticalLayout {
                            padding: 12px;
                            spacing: 4px;
                            HorizontalLayout {
                                Text { text: row.name; color: #f2f5f8; font-size: 15px; font-weight: 600; }
                                Text { text: row.value; color: row.stale ? #e8bd6a : #83d6bb; horizontal-alignment: right; }
                            }
                            if row.has-percent: Rectangle {
                                height: 4px;
                                background: #384657;
                                border-radius: 2px;
                                Rectangle { width: parent.width * row.percent / 100; height: parent.height; background: row.stale ? #c59d56 : #65c4a2; border-radius: 2px; }
                            }
                            Text { text: row.detail; color: #b4c1cf; font-size: 12px; wrap: word-wrap; }
                            Text { text: row.source; color: #8496a9; font-size: 11px; }
                        }
                    }
                }
            }
            Text { text: root.status; color: #a6b4c3; font-size: 11px; wrap: word-wrap; }
            Rectangle {
                background: #222e3b;
                border-radius: 8px;
                height: 102px;
                VerticalLayout {
                    padding: 12px;
                    spacing: 5px;
                    Text { text: "Доступ к USB-приёмникам"; color: #f2f5f8; font-weight: 600; }
                    Text { text: "Если нужны права доступа, установите правила udev по инструкции в README. После этого переподключите приёмник. Запускать приложение через sudo не нужно."; color: #b4c1cf; font-size: 12px; wrap: word-wrap; }
                }
            }
            Button { text: "Обновить показания"; clicked => { root.refresh(); } }
        }
    }
}

pub fn create(shared: Arc<Mutex<Snapshot>>, tx: mpsc::Sender<Command>) -> anyhow::Result<DetailsWindow> {
    let window = DetailsWindow::new()?;
    window.on_refresh(move || { let _ = tx.send(Command::Refresh); });
    window.window().on_close_requested(|| slint::CloseRequestResponse::HideWindow);
    update(&window, &shared);
    Ok(window)
}

pub fn update(window: &DetailsWindow, shared: &Arc<Mutex<Snapshot>>) {
    let Ok(s) = shared.lock() else { return; };
    let rows: Vec<_> = s.visible_devices().map(|d| DeviceRow {
        name: d.name.as_str().into(), value: d.label().into(), detail: d.detail.as_str().into(),
        source: if let Some(reading_time) = d.reading_time() {
            format!("{} · {reading_time}", d.source)
        } else {
            d.source.clone()
        }.into(),
        percent: d.percent.unwrap_or_default() as f32,
        has_percent: d.percent.is_some(),
        stale: matches!(d.state, crate::model::State::Stale | crate::model::State::Offline),
    }).collect();
    window.set_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    let status = if s.warnings.is_empty() { "Автообновление каждые 30 секунд. Недоступный заряд не означает 0%.".into() } else { format!("Автообновление: 30 с. {}", s.warnings.join(" · ")) };
    window.set_status(SharedString::from(status));
}
