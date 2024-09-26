use core::f32;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use eframe::egui::panel::Side;
use eframe::egui::{
    Align2, FontFamily, FontId, KeyboardShortcut, Pos2, Sense, Vec2, ViewportCommand, Visuals,
};
use eframe::{egui, Storage, Theme};
use egui_plot::{log_grid_spacer, GridMark, Legend, Line, Plot, PlotPoint, PlotPoints};
use egui_theme_switch::{ThemePreference, ThemeSwitch};
use preferences::Preferences;
use serde::{Deserialize, Serialize};
use serialport::{DataBits, FlowControl, Parity, StopBits};

use crate::data::{DataContainer, SerialDirection};
use crate::serial::{clear_serial_settings, save_serial_settings, Device, SerialDevices};
use crate::toggle::toggle;
use crate::FileOptions;
use crate::{APP_INFO, PREFS_KEY};

const MAX_FPS: f64 = 60.0;

const DEFAULT_FONT_ID: FontId = FontId::new(14.0, FontFamily::Monospace);
pub const RIGHT_PANEL_WIDTH: f32 = 350.0;
const BAUD_RATES: &[u32] = &[
    300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 74880, 115200, 230400, 128000, 460800,
    576000, 921600,
];

const SAVE_FILE_SHORTCUT: KeyboardShortcut =
    KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S);

// bitOr is not const, so we use plus
const SAVE_PLOT_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(
    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    egui::Key::S,
);

const CLEAR_PLOT_SHORTCUT: KeyboardShortcut =
    KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::X);

#[derive(Clone)]
#[allow(unused)]
pub enum Print {
    Empty,
    Message(String),
    Error(String),
    Debug(String),
    Ok(String),
}

#[derive(PartialEq)]
pub enum WindowFeedback {
    None,
    Waiting,
    Clear,
    Cancel,
}

impl Print {
    pub fn scroll_area_message(
        &self,
        gui_conf: &GuiSettingsContainer,
    ) -> Option<ScrollAreaMessage> {
        match self {
            Print::Empty => None,
            Print::Message(s) => {
                let color = if gui_conf.dark_mode {
                    egui::Color32::WHITE
                } else {
                    egui::Color32::BLACK
                };
                Some(ScrollAreaMessage {
                    label: "[MSG] ".to_owned(),
                    content: s.to_owned(),
                    color,
                })
            }
            Print::Error(s) => {
                let color = egui::Color32::RED;
                Some(ScrollAreaMessage {
                    label: "[ERR] ".to_owned(),
                    content: s.to_owned(),
                    color,
                })
            }
            Print::Debug(s) => {
                let color = if gui_conf.dark_mode {
                    egui::Color32::YELLOW
                } else {
                    egui::Color32::LIGHT_RED
                };
                Some(ScrollAreaMessage {
                    label: "[DBG] ".to_owned(),
                    content: s.to_owned(),
                    color,
                })
            }
            Print::Ok(s) => {
                let color = egui::Color32::GREEN;
                Some(ScrollAreaMessage {
                    label: "[OK] ".to_owned(),
                    content: s.to_owned(),
                    color,
                })
            }
        }
    }
}

#[allow(dead_code)]
pub struct ScrollAreaMessage {
    label: String,
    content: String,
    color: egui::Color32,
}

pub fn print_to_console(print_lock: &Arc<RwLock<Vec<Print>>>, message: Print) {
    match print_lock.write() {
        Ok(mut write_guard) => {
            write_guard.push(message);
        }
        Err(e) => {
            println!("Error while writing to print_lock: {}", e);
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct GuiSettingsContainer {
    pub device: String,
    pub baud: u32,
    pub debug: bool,
    pub x: f32,
    pub y: f32,
    pub save_absolute_time: bool,
    pub dark_mode: bool,
    pub theme_preference: ThemePreference,
}

impl Default for GuiSettingsContainer {
    fn default() -> Self {
        Self {
            device: "".to_string(),
            baud: 115_200,
            debug: true,
            x: 1600.0,
            y: 900.0,
            save_absolute_time: false,
            dark_mode: true,
            theme_preference: ThemePreference::System,
        }
    }
}

pub fn load_gui_settings() -> GuiSettingsContainer {
    GuiSettingsContainer::load(&APP_INFO, PREFS_KEY).unwrap_or_else(|_| {
        let gui_settings = GuiSettingsContainer::default();
        // save default settings
        if gui_settings.save(&APP_INFO, PREFS_KEY).is_err() {
            println!("failed to save gui_settings");
        }
        gui_settings
    })
}

pub fn load_global_font(ctx: &egui::Context) {
    let mut fonts = eframe::egui::FontDefinitions::default();

    // Install my own font (maybe supporting non-latin characters):
    fonts.font_data.insert(
        "msyh".to_owned(),
        eframe::egui::FontData::from_static(include_bytes!("C:\\Windows\\Fonts\\msyh.ttc")),
    ); // .ttf and .otf supported

    // Put my font first (highest priority):
    fonts
        .families
        .get_mut(&eframe::egui::FontFamily::Proportional)
        .unwrap()
        .insert(0, "msyh".to_owned());

    // Put my font as last fallback for monospace:
    fonts
        .families
        .get_mut(&eframe::egui::FontFamily::Monospace)
        .unwrap()
        .push("msyh".to_owned());

    // let mut ctx = egui::CtxRef::default();
    ctx.set_fonts(fonts);
}

pub struct MyApp {
    connected_to_device: bool,
    command: String,
    device: String,
    old_device: String,
    device_idx: usize,
    serial_devices: SerialDevices,
    plotting_range: usize,
    start_freq: String,
    end_freq: String,
    empty_freq: String,
    empty_qv: String,
    sample_name: String,
    sample_size: String,
    sample_freq: String,
    sample_qv: String,
    sample_dk: String,
    sample_df: String,
    select_cal_path: String,
    plot_serial_display_ratio: f32,
    console: Vec<Print>,
    picked_path: PathBuf,
    plot_location: Option<egui::Rect>,
    data: DataContainer,
    gui_conf: GuiSettingsContainer,
    print_lock: Arc<RwLock<Vec<Print>>>,
    device_lock: Arc<RwLock<Device>>,
    devices_lock: Arc<RwLock<Vec<String>>>,
    connected_lock: Arc<RwLock<bool>>,
    data_lock: Arc<RwLock<DataContainer>>,
    names_tx: Sender<Vec<String>>,
    save_tx: Sender<FileOptions>,
    send_tx: Sender<String>,
    clear_tx: Sender<bool>,
    history: Vec<String>,
    index: usize,
    eol: String,
    show_sent_cmds: bool,
    show_timestamps: bool,
    save_raw: bool,
    show_warning_window: WindowFeedback,
    do_not_show_clear_warning: bool,
}

#[allow(clippy::too_many_arguments)]
impl MyApp {
    pub fn new(
        print_lock: Arc<RwLock<Vec<Print>>>,
        data_lock: Arc<RwLock<DataContainer>>,
        device_lock: Arc<RwLock<Device>>,
        devices_lock: Arc<RwLock<Vec<String>>>,
        devices: SerialDevices,
        connected_lock: Arc<RwLock<bool>>,
        gui_conf: GuiSettingsContainer,
        names_tx: Sender<Vec<String>>,
        save_tx: Sender<FileOptions>,
        send_tx: Sender<String>,
        clear_tx: Sender<bool>,
    ) -> Self {
        Self {
            connected_to_device: false,
            picked_path: PathBuf::new(),
            device: "".to_string(),
            old_device: "".to_string(),
            data: DataContainer::default(),
            console: vec![Print::Message(
                "waiting for serial connection..,".to_owned(),
            )],
            connected_lock,
            device_lock,
            devices_lock,
            device_idx: 0,
            serial_devices: devices,
            print_lock,
            gui_conf,
            data_lock,
            names_tx,
            save_tx,
            send_tx,
            clear_tx,
            plotting_range: usize::MAX,
            start_freq: "".to_string(),
            end_freq: "".to_string(),
            empty_freq: "0".to_string(),
            empty_qv: "0".to_string(),
            sample_name: "".to_string(),
            sample_size: "".to_string(),
            sample_freq: "0".to_string(),
            sample_qv: "0".to_string(),
            sample_dk: "0".to_string(),
            sample_df: "0".to_string(),
            select_cal_path: "".to_string(),
            plot_serial_display_ratio: 0.45,
            command: "".to_string(),
            show_sent_cmds: true,
            show_timestamps: true,
            save_raw: false,
            eol: "\\r\\n".to_string(),
            history: vec![],
            index: 0,
            plot_location: None,
            do_not_show_clear_warning: false,
            show_warning_window: WindowFeedback::None,
        }
    }

    pub fn clear_warning_window(&mut self, ctx: &egui::Context) -> WindowFeedback {
        let mut window_feedback = WindowFeedback::Waiting;
        egui::Window::new("Attention!")
            .fixed_pos(Pos2 { x: 800.0, y: 450.0 })
            .fixed_size(Vec2 { x: 400.0, y: 200.0 })
            .anchor(Align2::CENTER_CENTER, Vec2 { x: 0.0, y: 0.0 })
            .collapsible(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    ui.label("Changing devices will clear all data.");
                    ui.label("How do you want to proceed?");
                    ui.add_space(20.0);
                    ui.checkbox(&mut self.do_not_show_clear_warning, "Remember my decision.");
                    ui.add_space(20.0);
                    ui.horizontal(|ui| {
                        ui.add_space(130.0);
                        if ui.button("Continue & Clear").clicked() {
                            window_feedback = WindowFeedback::Clear;
                        }
                        if ui.button("Cancel").clicked() {
                            window_feedback = WindowFeedback::Cancel;
                        }
                    });
                    ui.add_space(5.0);
                });
            });
        window_feedback
    }

    fn console_text(&self, packet: &crate::data::Packet) -> Option<String> {
        match (self.show_sent_cmds, self.show_timestamps, &packet.direction) {
            (true, true, _) => Some(format!(
                "[{}] t + {:.3}s: {}\n",
                packet.direction,
                packet.relative_time as f32 / 1000.0,
                packet.payload
            )),
            (true, false, _) => Some(format!("[{}]: {}\n", packet.direction, packet.payload)),
            (false, true, SerialDirection::Receive) => Some(format!(
                "t + {:.3}s: {}\n",
                packet.relative_time as f32 / 1000.0,
                packet.payload
            )),
            (false, false, SerialDirection::Receive) => Some(packet.payload.clone() + "\n"),
            (_, _, _) => None,
        }
    }

    fn draw_central_panel(&mut self, ctx: &egui::Context) {
        load_global_font(&ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            let left_border = 10.0;

            let panel_height = ui.available_size().y;
            let height = ui.available_size().y * self.plot_serial_display_ratio;
            let plots_height = height;
            // need to subtract 12.0, this seems to be the height of the separator of two adjacent plots
            let plot_height =
                plots_height / (self.serial_devices.number_of_plots[self.device_idx] as f32) - 12.0;
            let top_spacing = 5.0;
            let width = ui.available_size().x - 2.0 * left_border - RIGHT_PANEL_WIDTH;

            ui.add_space(top_spacing);
            ui.horizontal(|ui| {
                ui.add_space(left_border);
                ui.vertical(|ui| {
                    if let Ok(read_guard) = self.data_lock.read() {
                        self.data = read_guard.clone();
                    }

                    let mut graphs: Vec<Vec<PlotPoint>> = vec![vec![]; self.data.dataset.len()];
                    let window = self.data.dataset[0]
                        .len()
                        .saturating_sub(self.plotting_range);

                    for (i, time) in self.data.time[window..].iter().enumerate() {
                        let x = *time as f64 / 1000.0;
                        for (graph, data) in graphs.iter_mut().zip(&self.data.dataset) {
                            if self.data.time.len() == data.len() {
                                if let Some(y) = data.get(i + window) {
                                    graph.push(PlotPoint { x, y: *y as f64 });
                                }
                            }
                        }
                    }

                    let t_fmt =
                        |x: GridMark, _range: &RangeInclusive<f64>| format!("{:4.2} s", x.value);

                    let plots_ui = ui.vertical(|ui| {
                        for graph_idx in 0..self.serial_devices.number_of_plots[self.device_idx] {
                            if graph_idx != 0 {
                                ui.separator();
                            }

                            let signal_plot = Plot::new(format!("data-{graph_idx}"))
                                .height(plot_height)
                                .width(width)
                                .legend(Legend::default())
                                .x_grid_spacer(log_grid_spacer(10))
                                .y_grid_spacer(log_grid_spacer(10))
                                .x_axis_formatter(t_fmt);

                            let plot_inner = signal_plot.show(ui, |signal_plot_ui| {
                                for (i, graph) in graphs.iter().enumerate() {
                                    // this check needs to be here for when we change devices (not very elegant)
                                    if i < self.serial_devices.labels[self.device_idx].len() {
                                        signal_plot_ui.line(
                                            Line::new(PlotPoints::Owned(graph.to_vec())).name(
                                                &self.serial_devices.labels[self.device_idx][i],
                                            ),
                                        );
                                    }
                                }
                            });

                            self.plot_location = Some(plot_inner.response.rect);
                        }
                        let separator_response = ui.separator();
                        let separator = ui
                            .interact(
                                separator_response.rect,
                                separator_response.id,
                                Sense::click_and_drag(),
                            )
                            .on_hover_cursor(egui::CursorIcon::ResizeVertical);

                        let resize_y = separator.drag_delta().y;

                        if separator.double_clicked() {
                            self.plot_serial_display_ratio = 0.45;
                        }
                        self.plot_serial_display_ratio = (self.plot_serial_display_ratio
                            + resize_y / panel_height)
                            .clamp(0.1, 0.9);

                        ui.add_space(top_spacing);
                    });

                    let serial_height = panel_height
                        - plots_ui.response.rect.height()
                        - left_border * 2.0
                        - top_spacing;

                    let num_rows = self.data.raw_traffic.len();
                    let row_height = ui.text_style_height(&egui::TextStyle::Body);

                    let color = if self.gui_conf.dark_mode {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::BLACK
                    };

                    egui::ScrollArea::vertical()
                        .id_source("serial_output")
                        .auto_shrink([false; 2])
                        .stick_to_bottom(true)
                        .enable_scrolling(true)
                        .max_height(serial_height - top_spacing)
                        .min_scrolled_height(serial_height - top_spacing)
                        .max_width(width)
                        .show_rows(ui, row_height, num_rows, |ui, row_range| {
                            let content: String = row_range
                                .into_iter()
                                .flat_map(|i| {
                                    if self.data.raw_traffic.is_empty() {
                                        None
                                    } else {
                                        self.console_text(&self.data.raw_traffic[i])
                                    }
                                })
                                .collect();
                            ui.add(
                                egui::TextEdit::multiline(&mut content.as_str())
                                    .font(DEFAULT_FONT_ID) // for cursor height
                                    .lock_focus(true)
                                    .text_color(color)
                                    .desired_width(width),
                            );
                        });
                    ctx.request_repaint()
                });
                ui.add_space(left_border);
            });
        });
    }

    fn draw_side_panel(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let mut init = false;
        egui::SidePanel::new(Side::Right, "settings panel")
            .min_width(RIGHT_PANEL_WIDTH)
            .max_width(RIGHT_PANEL_WIDTH)
            .resizable(false)
            //.default_width(right_panel_width)
            .show(ctx, |ui| {
                ui.add_enabled_ui(true, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("谐振法扫频源控制程序");
                        self.paint_connection_indicator(ui);
                    });

                    let devices: Vec<String> = if let Ok(read_guard) = self.devices_lock.read() {
                        read_guard.clone()
                    } else {
                        vec![]
                    };

                    if !devices.contains(&self.device) {
                        self.device.clear();
                    }

                    ui.add_space(10.0);
                    ui.label("设备列表");

                    let old_name = self.device.clone();
                    ui.horizontal(|ui| {
                        let dev_text = self.device.replace("/dev/tty.", "");
                        ui.horizontal(|ui| {
                            if self.connected_to_device {
                                ui.disable();
                            }
                            let _response = egui::ComboBox::from_id_source("Device")
                                .selected_text(dev_text)
                                .width(RIGHT_PANEL_WIDTH * 0.95 - 70.0)
                                .show_ui(ui, |ui| {
                                    devices
                                        .into_iter()
                                        // on macOS each device appears as /dev/tty.* and /dev/cu.*
                                        // we only display the /dev/tty.* here
                                        .filter(|dev| !dev.contains("/dev/cu."))
                                        .for_each(|dev| {
                                            // this makes the names shorter in the UI on UNIX and UNIX-like platforms
                                            let dev_text = dev.replace("/dev/tty.", "");
                                            ui.selectable_value(&mut self.device, dev, dev_text);
                                        });
                                })
                                .response;
                            // let selected_new_device = response.changed();  //somehow this does not work
                            // if selected_new_device {
                            if old_name != self.device {
                                if !self.data.time.is_empty() {
                                    self.show_warning_window = WindowFeedback::Waiting;
                                    self.old_device = old_name;
                                } else {
                                    self.show_warning_window = WindowFeedback::Clear;
                                }
                            }
                        });
                        match self.show_warning_window {
                            WindowFeedback::None => {}
                            WindowFeedback::Waiting => {
                                self.show_warning_window = self.clear_warning_window(ctx);
                            }
                            WindowFeedback::Clear => {
                                // new device selected, check in previously used devices
                                let mut device_is_already_saved = false;
                                for (idx, dev) in self.serial_devices.devices.iter().enumerate() {
                                    if dev.name == self.device {
                                        // this is the device!
                                        self.device = dev.name.clone();
                                        self.device_idx = idx;
                                        init = true;
                                        device_is_already_saved = true;
                                    }
                                }
                                if !device_is_already_saved {
                                    // create new device in the archive
                                    let mut device = Device::default();
                                    device.name = self.device.clone();
                                    self.serial_devices.devices.push(device);
                                    self.serial_devices.number_of_plots.push(1);
                                    self.serial_devices
                                        .labels
                                        .push(vec!["Column 0".to_string()]);
                                    self.device_idx = self.serial_devices.devices.len() - 1;
                                    save_serial_settings(&self.serial_devices);
                                }
                                self.clear_tx
                                    .send(true)
                                    .expect("failed to send clear after choosing new device");
                                // need to clear the data here such that we don't get errors in the gui (plot)
                                self.data = DataContainer::default();
                                self.show_warning_window = WindowFeedback::None;
                            }
                            WindowFeedback::Cancel => {
                                self.device = self.old_device.clone();
                                self.show_warning_window = WindowFeedback::None;
                            }
                        }
                        let connect_text = if self.connected_to_device {
                            "关闭连接"
                        } else {
                            "启动连接"
                        };
                        if ui.button(connect_text).clicked() {
                            if let Ok(mut device) = self.device_lock.write() {
                                if self.connected_to_device {
                                    device.name.clear();
                                } else {
                                    device.name =
                                        self.serial_devices.devices[self.device_idx].name.clone();
                                    device.baud_rate =
                                        self.serial_devices.devices[self.device_idx].baud_rate;
                                }
                            }
                        }
                    });
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.label("起始扫描频率: ");
                        ui.add_space(63.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.start_freq)
                                .hint_text("MHz,输入起始频率"),
                        );
                    });
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.label("截至扫描频率: ");
                        ui.add_space(63.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.start_freq)
                                .hint_text("MHz,输入截至频率"),
                        );
                    });
                    ui.add_space(10.0);

                    egui::Grid::new("upper")
                        .num_columns(2)
                        .spacing(Vec2 { x: 10.0, y: 10.0 })
                        .striped(true)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_space(90.0);
                                ui.add(egui::widgets::Button::new(egui::RichText::new(
                                    "点击搜寻空腔谐振状态",
                                )));
                            });
                            ui.end_row();

                            ui.horizontal(|ui| {
                                ui.label("空腔谐振频率(MHz): ");
                                ui.add_space(112.0);
                                ui.label(format!("{}", self.empty_freq));
                            });
                            ui.end_row();

                            ui.horizontal(|ui| {
                                ui.label("空腔Q值: ");
                                ui.add_space(177.0);
                                ui.label(format!("{}", self.empty_qv));
                            });
                            ui.end_row();
                            ui.end_row();
                            ui.heading("样品信息");
                            ui.end_row();
                            ui.horizontal(|ui| {
                                ui.label("样品名称: ");
                                ui.add_space(63.0);
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.sample_name)
                                        .hint_text("输入样品名称"),
                                );
                            });
                            ui.end_row();

                            ui.horizontal(|ui| {
                                ui.label("样品尺寸(mm): ");
                                ui.add_space(30.0);
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.sample_size)
                                        .hint_text("输入样品尺寸"),
                                )
                                .on_hover_text(
                                    "片状样品输入厚底，棒状样品输入直径，其他样品输入体积",
                                );
                            });
                            ui.end_row();

                            ui.horizontal(|ui| {
                                ui.add_space(90.0);
                                ui.add(egui::widgets::Button::new(egui::RichText::new(
                                    "点击搜寻样品谐振状态",
                                )));
                            });
                            ui.end_row();

                            ui.horizontal(|ui| {
                                ui.label("样品谐振频率(MHz): ");
                                ui.add_space(112.0);
                                ui.label(format!("{}", self.sample_freq));
                            });
                            ui.end_row();

                            ui.horizontal(|ui| {
                                ui.label("样品Q值: ");
                                ui.add_space(177.0);
                                ui.label(format!("{}", self.sample_qv));
                            });
                            ui.end_row();

                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.select_cal_path)
                                        .hint_text("                      未指定计算程序"),
                                );
                                ui.add_space(5.0);
                                if ui
                                    .button("选择")
                                    .on_hover_text("选择对应计算程序")
                                    .clicked()
                                {
                                    if let Some(path) = rfd::FileDialog::new()
                                        .add_filter("calculate", &["exe"])
                                        .pick_file()
                                    {
                                        self.select_cal_path = path.display().to_string();
                                    }
                                }
                            });
                            ui.end_row();

                            ui.horizontal(|ui| {
                                ui.label("样品介电常数: ");
                                ui.add_space(150.0);
                                ui.label(format!("{}", self.sample_dk));
                            });
                            ui.end_row();

                            ui.horizontal(|ui| {
                                ui.label("样品介电损耗: ");
                                ui.add_space(150.0);
                                ui.label(format!("{}", self.sample_df));
                            });
                            ui.end_row();
                        });

                    ui.add_space(25.0);
                    self.gui_conf.dark_mode = ui.visuals() == &Visuals::dark();
                    ui.horizontal(|ui| {
                        if ui.button("Clear Device History").clicked() {
                            self.serial_devices = SerialDevices::default();
                            self.device.clear();
                            self.device_idx = 0;
                            clear_serial_settings();
                        }
                        if ui.button("Reset Labels").clicked() {
                            self.serial_devices.labels[self.device_idx] = self.data.names.clone();
                        }
                    });
                    if self.data.names.len() == 1 {
                        ui.label("Detected 1 Dataset:");
                    } else {
                        ui.label(format!("Detected {} Datasets:", self.data.names.len()));
                    }
                    ui.add_space(5.0);
                    for i in 0..self.data.names.len().min(10) {
                        // if init, set names to what has been stored in the device last time
                        if init {
                            self.names_tx
                                .send(self.serial_devices.labels[self.device_idx].clone())
                                .expect("Failed to send names");
                            init = false;
                        }
                        if self.serial_devices.labels[self.device_idx].len() <= i {
                            break;
                        }

                        if ui
                            .add(
                                egui::TextEdit::singleline(
                                    &mut self.serial_devices.labels[self.device_idx][i],
                                )
                                .desired_width(0.95 * RIGHT_PANEL_WIDTH),
                            )
                            .on_hover_text("Use custom names for your Datasets.")
                            .changed()
                        {
                            self.names_tx
                                .send(self.serial_devices.labels[self.device_idx].clone())
                                .expect("Failed to send names");
                        };
                    }
                    if self.data.names.len() > 10 {
                        ui.label("Only renaming up to 10 Datasets is currently supported.");
                    }

                    ui.add_space(25.0);
                    if ui
                        .add(ThemeSwitch::new(&mut self.gui_conf.theme_preference))
                        .changed()
                    {
                        // do nothing, for now...
                    };
                    // always set dark mode
                    let theme = match self.gui_conf.theme_preference {
                        ThemePreference::Dark => Theme::Dark,
                        ThemePreference::Light => Theme::Light,
                        ThemePreference::System => {
                            let eframe_system_theme = frame.info().system_theme;
                            eframe_system_theme.unwrap_or(Theme::Dark)
                        }
                    };
                    ctx.set_visuals(theme.egui_visuals());
                    ctx.send_viewport_cmd(ViewportCommand::SetTheme(
                        self.gui_conf.theme_preference.into(),
                    ));
                });

                if let Ok(read_guard) = self.print_lock.read() {
                    self.console = read_guard.clone();
                }
                let num_rows = self.console.len();
                let row_height = ui.text_style_height(&egui::TextStyle::Body);

                ui.add_space(20.0);
                ui.separator();
                ui.label("Debug Info:");
                ui.add_space(5.0);
                egui::ScrollArea::vertical()
                    .id_source("console_scroll_area")
                    .auto_shrink([false; 2])
                    .stick_to_bottom(true)
                    .max_height(row_height * 15.5)
                    .show_rows(ui, row_height, num_rows, |ui, _row_range| {
                        let content: String = self
                            .console
                            .iter()
                            .flat_map(|row| row.scroll_area_message(&self.gui_conf))
                            .map(|msg| msg.label + msg.content.as_str())
                            .collect::<Vec<_>>()
                            .join("\n");
                        // we need to add it as one multiline object, such that we can select and copy
                        // text over multiple lines
                        ui.add(
                            egui::TextEdit::multiline(&mut content.as_str())
                                .font(DEFAULT_FONT_ID) // for cursor height
                                .lock_focus(true), // TODO: add a layouter to highlight the labels
                        );
                    });
            });
    }

    fn paint_connection_indicator(&self, ui: &mut egui::Ui) {
        let (color, color_stroke) = if !self.connected_to_device {
            ui.add(egui::Spinner::new());
            (egui::Color32::DARK_RED, egui::Color32::RED)
        } else {
            (egui::Color32::DARK_GREEN, egui::Color32::GREEN)
        };

        let radius = ui.spacing().interact_size.y * 0.375;
        let center = egui::pos2(
            ui.next_widget_position().x + ui.spacing().interact_size.x * 0.5,
            ui.next_widget_position().y,
        );
        ui.painter()
            .circle(center, radius, color, egui::Stroke::new(1.0, color_stroke));
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Ok(read_guard) = self.connected_lock.read() {
            self.connected_to_device = *read_guard;
        }

        self.draw_central_panel(ctx);
        self.draw_side_panel(ctx, frame);

        self.gui_conf.x = ctx.used_size().x;
        self.gui_conf.y = ctx.used_size().y;

        // Check for returned screenshot:
        let screenshot = ctx.input(|i| {
            for event in &i.raw.events {
                if let egui::Event::Screenshot { image, .. } = event {
                    return Some(image.clone());
                }
            }
            None
        });

        if let (Some(screenshot), Some(plot_location)) = (screenshot, self.plot_location) {
            if let Some(mut path) = rfd::FileDialog::new().save_file() {
                path.set_extension("png");

                // for a full size application, we should put this in a different thread,
                // so that the GUI doesn't lag during saving

                let pixels_per_point = ctx.pixels_per_point();
                let plot = screenshot.region(&plot_location, Some(pixels_per_point));
                // save the plot to png
                image::save_buffer(
                    &path,
                    plot.as_raw(),
                    plot.width() as u32,
                    plot.height() as u32,
                    image::ColorType::Rgba8,
                )
                .unwrap();
                eprintln!("Image saved to {path:?}.");
            }
        }

        std::thread::sleep(Duration::from_millis((1000.0 / MAX_FPS) as u64));
    }

    fn save(&mut self, _storage: &mut dyn Storage) {
        save_serial_settings(&self.serial_devices);
        if let Err(err) = self.gui_conf.save(&APP_INFO, PREFS_KEY) {
            println!("gui settings save failed: {:?}", err);
        }
    }
}
