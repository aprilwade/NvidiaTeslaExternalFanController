use std::error::Error;
use std::thread;

use hidapi::HidApi;
use nvml_wrapper::{Nvml, enum_wrappers::device::TemperatureSensor};
use structopt::StructOpt;


#[derive(Clone, Debug)]
struct FanSpeedTable {
    table: Vec<(f64, u8)>,
}

impl FanSpeedTable {
    fn new(mut table: Vec<(f64, u8)>) -> Self {
        table.sort_by(|(a, _), (b, _)| a.total_cmp(b));
        FanSpeedTable {
            table,
        }
    }

    fn lookup_speed(&self, power_usage: f64) -> u8 {
        let power_usage = power_usage.clamp(0.0, 1.0);

        let (upper_usage, upper_speed) = self.table.iter()
            .find(|(pct, _)| power_usage < *pct)
            .copied()
            .unwrap_or((1.0, 255));
        let (lower_usage, lower_speed) = self.table.iter()
            .rev()
            .find(|(pct, _)| power_usage > *pct)
            .copied()
            .unwrap_or((0.0, 0));

        let usage_pct = (power_usage - lower_usage) as f64 / (upper_usage - lower_usage) as f64;
        (upper_speed as f64 * usage_pct + lower_speed as f64 * (1.0 - usage_pct)) as u8
    }
}

impl std::str::FromStr for FanSpeedTable {
    type Err = Box<dyn std::error::Error>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.split(',')
            .enumerate()
            .map(|(i, s)| {
                let (before, after) = s.split_once(':')
                    .ok_or_else(|| format!(
                        "Missing ':' in entry {}: \
                        Each entry needs a seperate power usage percent and fan speed",
                        i
                    ))?;
                let power_usage: f64 = before.parse()?;
                if power_usage < 0.0 || power_usage > 1.0 {
                    Err("power usage must be between 0.0 and 1.0")?
                }
                let fan_speed: u8 = after.parse()?;
                Ok((power_usage, fan_speed))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(FanSpeedTable::new)
    }

}

// 10% @   0/255 => 37c
// 12% @   0/255 => 44c
//
// 35% @   0/255 => 70c
// 33% @   0/255 => 68c
// 32% @   0/255 => 66c
//
// 40% @  50/255 => 70c
// 40% @  65/255 => 66c
// 40% @  75/255 => 62c
//
// 60% @ 120/255 => 65c
//
// 70% @ 175/255 => 65c
//
// 80% @ 210/255 => 67c
//
// 98% @ 255/255 => 68c
// <= .3 : 0
// .3 : 30
// .4 : 70
// .6 : 120
// .7 : 170
// .8 : 210
// >= .95 : 255
const DEFAULT_FAN_SPEED: &[(f64, u8)] = &[
    (0.3, 0),
    (0.4, 70),
    (0.6, 120),
    (0.7, 170),
    (0.8, 210),
    (0.95, 255),
];

fn default_fan_speed_table() -> FanSpeedTable {
    FanSpeedTable::new(DEFAULT_FAN_SPEED.to_vec())
}

/*
#[derive(Copy, Clone, Debug)]
struct PidParams {
    p: f64,
    i: f64,
    d: f64,
}

impl std::str::FromStr for PidParams {
    type Err = Box<dyn std::error::Error>;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split(':');
        Ok(PidParams {
            p: parts.next().ok_or("Too few parts; missing p")?.parse()?,
            i: parts.next().ok_or("Too few parts; missing i")?.parse()?,
            d: parts.next().ok_or("Too few parts; missing d")?.parse()?,
        })
    }
}
*/

struct CircleBuf<T> {
    n: usize,
    buf: T,
}

impl<T> CircleBuf<T> {
    fn new(buf: T) -> Self {
        CircleBuf {
            n: 0,
            buf,
        }
    }

    fn push<E>(&mut self, e: E)
        where T: AsMut<[E]>,
    {
        let buf = self.buf.as_mut();
        self.n = self.n % buf.len();
        buf[self.n] = e;
        self.n += 1;
    }
}

impl<E, T> std::ops::Deref for CircleBuf<T>
    where T: std::ops::Deref<Target = [E]>
{
    type Target = [E];
    fn deref(&self) -> &Self::Target {
        self.buf.deref()
    }
}


#[derive(Debug, Clone, StructOpt)]
#[structopt(
    name = "fan_controller",
    about = "Updates the fan controller of the GPU's temperature.",
    rename_all = "kebab-case",
)]
struct Args {
    #[structopt(short, long, default_value = "GPU-b60cae4e-f524-14a8-2233-2dc2126b6754")]
    uuid: String,

    #[structopt(short, long)]
    speed_override: Option<u8>,

    #[structopt(short = "t", long, default_value = "5.0")]
    update_interval: f64,

    #[structopt(short, long)]
    fan_curve: Option<FanSpeedTable>,

    #[structopt(short, long)]
    logging: bool,
}

fn inner_main(args: Args) -> Result<(), Box<dyn Error>> {
    let fan_curve = args.fan_curve
        .unwrap_or_else(default_fan_speed_table);

    let mut hidapi = HidApi::new()
        .map_err(|e| format!("Failed to init HidApi: {}", e))?;

    let _ = hidapi.refresh_devices();
    if let Some(speed_override) = args.speed_override {
        let fan_controller = hidapi.open(0x1209, 0x0010)
            .map_err(|e| format!("Failed to find fan controller: {}", e))?;

        let mut buf = [0u8; 64];
        if cfg!(windows) {
            buf[0] = 1;
            buf[1] = 1;
            buf[2] = speed_override;
        } else {
            buf[0] = 1;
            buf[1] = speed_override;
        }
        fan_controller.write(&buf[..])
            .map_err(|e| format!("Error updating fan controller: {}", e))?;

        return Ok(())
    }

    let nvml = if cfg!(windows) {
        Nvml::init()
    } else {
        Nvml::builder()
            .lib_path("./libnvidia-ml.so".as_ref())
            .init()
    };
    let nvml = nvml
        .map_err(|e| format!("Failed to init NVML: {}", e))?;

    let gpu = nvml.device_by_uuid(&args.uuid[..])
        .map_err(|e| format!("Failed to find Tesla GPU: {}", e))?;

    if args.logging {
        println!(
            "{:?} - {} - {} - {}",
            gpu,
            gpu.name()?,
            gpu.uuid()?,
            gpu.temperature(TemperatureSensor::Gpu)?
        );
    }

    let temp = gpu.temperature(TemperatureSensor::Gpu)?;
    let power_usage = gpu.power_usage()?;
    let power_limit = gpu.power_management_limit()?;

    // We want to keep a 1 minute history
    let samples = (60.0 / args.update_interval).ceil() as usize;
    let mut temp_history = CircleBuf::new(vec![temp as u8; samples]);
    let mut power_history = CircleBuf::new(vec![power_usage as f64 / power_limit as f64; samples]);

    let mut prev_speed = None;

    let mut fan_controller = None;
    loop {
        thread::sleep(std::time::Duration::from_millis((args.update_interval * 1000.0) as u64));

        // The fan controller might get disconnected, so handle that potential
        // Ugh, this code is ugly :(
        let fan_controller_ref = match &mut fan_controller {
            Some(device) => device,
            None => {
                let _ = hidapi.refresh_devices();
                match hidapi.open(0x1209, 0x0010) {
                    Ok(device) => fan_controller.insert(device),
                    Err(e) => {
                        println!("Failed to find fan controller: {}", e);
                        continue
                    },
                }
            },
        };

        let speed = loop {
            let temp = match gpu.temperature(TemperatureSensor::Gpu) {
                Ok(temp) => temp,
                Err(e) => {
                    println!("Error updating fan controller: {}", e);
                    break 255
                },
            };
            let power_usage = match gpu.power_usage() {
                Ok(power_usage) => power_usage,
                Err(e) => {
                    println!("Error updating fan controller: {}", e);
                    break 255
                },
            };
            let power_limit = match gpu.power_management_limit() {
                Ok(power_limit) => power_limit,
                Err(e) => {
                    println!("Error updating fan controller: {}", e);
                    break 255
                },
            };

            temp_history.push(temp as u8);
            power_history.push(power_usage as f64 / power_limit as f64);
            let max_temp = *temp_history.iter().max().unwrap();

            // Safety condition in case we get run away temps
            if max_temp >= 77 {
                break 255
            }

            let average_power = power_history.iter().sum::<f64>() / power_history.len() as f64;
            let speed = fan_curve.lookup_speed(average_power);

            // If we're at or over 72 degrees, increase the fan speed just in case
            let adj_speed = if max_temp >= 72 {
                speed.saturating_add(50)
            } else {
                speed
            };

            if args.logging {
                println!(
                    "Avg power {:.1}, Max temp {}, Comp speed {}, Prev speed {}, Adj speed {}",
                    average_power * 100.0,
                    max_temp,
                    speed,
                    prev_speed.map(|i| i as i32).unwrap_or(-1),
                    adj_speed
                );
            }
            break adj_speed
        };

        // If the new speed is within +/- 5% of the old speed, don't report it
        if let Some(prev_speed) = prev_speed {
            if (speed as f64 - prev_speed as f64).abs() <= 12.75
                    // Make sure if we reach max speed, we report that (but only once)
                    && !(prev_speed != 0 && speed == 0)
                    && !(prev_speed != 255 && speed == 255) {
                // Do not update
                continue
            }
        }

        let mut buf = [0u8; 64];
        if cfg!(windows) {
            buf[0] = 1;
            buf[1] = 1;
            buf[2] = speed;
        } else {
            buf[0] = 1;
            buf[1] = speed;
        }
        match fan_controller_ref.write(&buf[..]) {
            Ok(_) => {
                println!("Setting speed to {}", speed);
                prev_speed = Some(speed);
            },
            Err(e) => {
                println!("Error updating fan controller: {}", e);
                fan_controller = None;
            },
        }
    }

    // Ok(())
}


fn main() {
    let args = Args::from_args();
    match inner_main(args) {
        Ok(()) => (),
        Err(e) => {
            println!("Error occurred: {}", e.to_string());
        },
    }
    /*
    let device = (0..nvml.device_count().unwrap())
        .map(|i| nvml.device_by_index(i).unwrap())
        .find(|device| device.uuid().unwrap() == "GPU-b60cae4e-f524-14a8-2233-2dc2126b6754")
        .unwrap();
    */

    // for i in 0..nvml.device_count().unwrap() {
    //     let device = nvml.device_by_index(i).unwrap();
    //     println!("{:?} - {} - {} - {}", device, device.name().unwrap(), device.uuid().unwrap(), device.temperature(TemperatureSensor::Gpu).unwrap());
    // }

    /*
    let arg: u8 = env::args().nth(1).unwrap().parse().unwrap();
    println!("{}", arg);

    let hidapi = HidApi::new().unwrap();
    for device in hidapi.device_list() {
        println!("{:?}", device.path());
    }
    let device = hidapi.open(0x1209, 0x0010).unwrap();
    println!("{:?}", device.check_error());
    println!("{:?}", device.get_product_string());
    let mut buf = [0u8; 64];
    buf[0] = 1;
    buf[1] = arg;
    let amt = device.write(&mut buf[..]).unwrap();
    println!("{}: {:?}", amt, &buf[..amt]);
    */

    /*
    loop {
        buf[0] = arg;
        buf[0] = arg;
        buf[0] = arg;
        let amt = device.write(&mut buf[..64]).unwrap();
        println!("{}: {:?}", amt, &buf[..amt]);
        // let amt = device.read(&mut buf).unwrap();
        // println!("{}: {:?}", amt, &buf[..amt]);
        // break
    }
    */

    // for device in hidapi.device_list() {
    //     if device.vendor_id() == 0x5555 && device.product_id() == 0x5555 {
    //         let device = device.open_device(&hidapi).unwrap();
    //     }
    //     // println!("{:?}", device.serial_number_raw());
    //     // println!("{:?}", device);
    // }

    // println!("Hello, world!");
}
