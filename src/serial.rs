use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use preferences::Preferences;
use serde::{Deserialize, Serialize};
use serialport::{DataBits, FlowControl, Parity, SerialPort, StopBits};

use crate::data::{get_epoch_ms, SerialDirection};
use crate::{print_to_console, Packet, Print, APP_INFO, PREFS_KEY_SERIAL};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerialDevices {
    pub device: Device,
    pub labels: Vec<String>,
}

impl Default for SerialDevices {
    fn default() -> Self {
        SerialDevices {
            device: Device::default(),
            labels: vec!["Column 0".to_string()],
        }
    }
}

pub fn load_serial_settings() -> SerialDevices {
    SerialDevices::load(&APP_INFO, PREFS_KEY_SERIAL).unwrap_or_else(|_| {
        let serial_configs = SerialDevices::default();
        // save default settings
        save_serial_settings(&serial_configs);
        serial_configs
    })
}

pub fn save_serial_settings(serial_configs: &SerialDevices) {
    if serial_configs.save(&APP_INFO, PREFS_KEY_SERIAL).is_err() {
        println!("failed to save gui_settings");
    }
}

pub fn clear_serial_settings() {
    let serial_configs = SerialDevices::default();
    if serial_configs.save(&APP_INFO, PREFS_KEY_SERIAL).is_err() {
        println!("failed to clear gui_settings");
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Device {
    pub name: String,
    pub baud_rate: u32,
    pub data_bits: DataBits,
    pub flow_control: FlowControl,
    pub parity: Parity,
    pub stop_bits: StopBits,
    pub timeout: Duration,
}

impl Default for Device {
    fn default() -> Self {
        Device {
            name: "".to_string(),
            baud_rate: 115200,
            data_bits: DataBits::Eight,
            flow_control: FlowControl::None,
            parity: Parity::None,
            stop_bits: StopBits::One,
            timeout: Duration::from_millis(0),
        }
    }
}

fn serial_write(
    port: &mut BufReader<Box<dyn SerialPort>>,
    cmd: &[u8],
) -> Result<usize, std::io::Error> {
    let write_port = port.get_mut();
    write_port.write(cmd)
}

fn serial_read(
    port: &mut BufReader<Box<dyn SerialPort>>,
    serial_buf: &mut String,
) -> Result<usize, std::io::Error> {
    port.read_line(serial_buf)
}

pub fn serial_thread(
    send_rx: Receiver<String>,
    // start_sweep: Arc<AtomicI64>,
    // stop_sweep: Arc<AtomicI64>,
    // step_sweep: Arc<AtomicI64>,
    raw_data_tx: Sender<Packet>,
    device_lock: Arc<RwLock<Device>>,
    devices_lock: Arc<RwLock<Vec<String>>>,
    print_lock: Arc<RwLock<Vec<Print>>>,
    connected_lock: Arc<RwLock<bool>>,
) {
    loop {
        let _not_awake = keepawake::Builder::default()
            .display(false)
            .reason("Serial Connection")
            .app_name("Serial Monitor")
            //.app_reverse_domain("io.github.myprog")
            .create();

        if let Ok(mut connected) = connected_lock.write() {
            *connected = false;
        }

        let device = get_device(&devices_lock, &device_lock);

        let mut port = match serialport::new(&device.name, device.baud_rate)
            .timeout(Duration::from_millis(100))
            .open()
        {
            Ok(p) => {
                if let Ok(mut connected) = connected_lock.write() {
                    *connected = true;
                }
                print_to_console(
                    &print_lock,
                    Print::Ok(format!(
                        "Connected to serial port: {} @ baud = {}",
                        device.name, device.baud_rate
                    )),
                );
                BufReader::new(p)
            }
            Err(err) => {
                if let Ok(mut write_guard) = device_lock.write() {
                    write_guard.name.clear();
                }
                print_to_console(
                    &print_lock,
                    Print::Error(format!("Error connecting: {}", err)),
                );
                continue;
            }
        };

        let t_zero = Instant::now();

        let _awake = keepawake::Builder::default()
            .display(true)
            .reason("Serial Connection")
            .app_name("Serial Monitor")
            //.app_reverse_domain("io.github.myprog")
            .create();

        // Send start_sweep, stop_sweep, and step_sweep information
        // let start_sweep_val = start_sweep.load(Ordering::Relaxed);
        // let stop_sweep_val = stop_sweep.load(Ordering::Relaxed);
        // let step_sweep_val = step_sweep.load(Ordering::Relaxed);
        // Send_Msg_Ka(&mut port, start_sweep_val, 0);
        // Send_Msg_Ka(&mut port, stop_sweep_val, 1);
        // Send_Msg_Ka(&mut port, step_sweep_val, 2);

        perform_writes(&mut port, &send_rx, &raw_data_tx, t_zero);

        'connected_loop: loop {
            let devices = available_devices();
            if let Ok(mut write_guard) = devices_lock.write() {
                *write_guard = devices.clone();
            }

            if let Some(message) = disconnected(&device, &devices, &device_lock) {
                print_to_console(&print_lock, message);
                break 'connected_loop;
            }

            perform_reads(&mut port, &raw_data_tx, t_zero);
        }
        std::mem::drop(port);
    }
}

fn available_devices() -> Vec<String> {
    serialport::available_ports()
        .unwrap()
        .iter()
        .map(|p| p.port_name.clone())
        .collect()
}

fn get_device(
    devices_lock: &Arc<RwLock<Vec<String>>>,
    device_lock: &Arc<RwLock<Device>>,
) -> Device {
    loop {
        let devices = available_devices();
        if let Ok(mut write_guard) = devices_lock.write() {
            *write_guard = devices.clone();
        }

        if let Ok(device) = device_lock.read() {
            if devices.contains(&device.name) {
                return device.clone();
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn disconnected(
    device: &Device,
    devices: &[String],
    device_lock: &Arc<RwLock<Device>>,
) -> Option<Print> {
    // disconnection by button press
    if let Ok(read_guard) = device_lock.read() {
        if device.name != read_guard.name {
            return Some(Print::Ok(format!(
                "Disconnected from serial port: {}",
                device.name
            )));
        }
    }

    // other types of disconnection (e.g. unplugging, power down)
    if !devices.contains(&device.name) {
        if let Ok(mut write_guard) = device_lock.write() {
            write_guard.name.clear();
        }
        return Some(Print::Error(format!(
            "Device has disconnected from serial port: {}",
            device.name
        )));
    }

    None
}

fn perform_writes(
    port: &mut BufReader<Box<dyn SerialPort>>,
    send_rx: &Receiver<String>,
    raw_data_tx: &Sender<Packet>,
    t_zero: Instant,
) {
    if let Ok(cmd) = send_rx.recv_timeout(Duration::from_millis(1)) {
        if let Err(e) = serial_write(port, cmd.as_bytes()) {
            println!("Error sending command: {e}");
            return;
        }

        let packet = Packet {
            relative_time: Instant::now().duration_since(t_zero).as_millis(),
            absolute_time: get_epoch_ms(),
            direction: SerialDirection::Send,
            payload: cmd,
        };
        raw_data_tx
            .send(packet)
            .expect("failed to send raw data (cmd)");
    }
}

fn perform_reads(
    port: &mut BufReader<Box<dyn SerialPort>>,
    raw_data_tx: &Sender<Packet>,
    t_zero: Instant,
) {
    let mut buf = "".to_string();
    match serial_read(port, &mut buf) {
        Ok(_) => {
            let delimiter = if buf.contains("\r\n") { "\r\n" } else { "\0\0" };
            buf.split_terminator(delimiter).for_each(|s| {
                let packet = Packet {
                    relative_time: Instant::now().duration_since(t_zero).as_millis(),
                    absolute_time: get_epoch_ms(),
                    direction: SerialDirection::Receive,
                    payload: s.to_owned(),
                };
                raw_data_tx.send(packet).expect("failed to send raw data");
            });
        }
        // Timeout is ok, just means there is no data to read
        Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
        Err(e) => {
            println!("Error reading: {:?}", e);
        }
    }
}
