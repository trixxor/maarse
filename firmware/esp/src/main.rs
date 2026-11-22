//! Må:rse firmware for ESP32-S3 + SX1262.
//!
//! * Heltec WiFi LoRa 32 V4 (with OLED display and PA amplifier)
//! * Seeed XIAO ESP32-S3 + Wio-SX1262
//!
//! The same behaviour as the nRF firmware: the companion protocol over USB
//! (USB-Serial-JTAG), modem mode for maarse-host, radio settings and
//! administrators in flash, a mailbox in flash and Bluetooth (Nordic UART
//! Service, the same GATT service and name as on nRF).
//!
//! Build: scripts/build-firmware.sh heltec-v4 (needs espup and espflash).

#![no_std]
#![no_main]

#[cfg(feature = "board-heltec-v4")]
mod battery;
#[cfg(feature = "ble")]
mod ble;
#[cfg(feature = "display")]
mod display;
#[cfg(feature = "display")]
mod display_ui;
#[cfg(feature = "display")]
#[allow(dead_code)] // generated: not every size is used
mod art;
mod ota;
mod radio;
mod sensor;
mod usb;
#[cfg(feature = "wifi")]
mod wifi;

use embassy_executor::Spawner;
use embassy_time::Delay;
#[allow(unused_imports)]
use embassy_time::Timer;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::rng::{Trng, TrngSource};
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_storage::FlashStorage;
use lora_phy::LoRa;
use lora_phy::iv::GenericSx126xInterfaceVariant;
use lora_phy::sx126x::{self, Sx126x, Sx1262, TcxoCtrlVoltage};
use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};
use static_cell::StaticCell;
use maarse_core::companion::RadioConfig;
use maarse_fw_common::node::Settings;
use maarse_fw_common::storage::{FlashSettings, MAIL_AREA};

esp_bootloader_esp_idf::esp_app_desc!();

/// Wi-Fi start attempts that did not survive, kept over a restart (RTC fast
/// memory). `wifi_start_task` counts up before starting Wi-Fi and back to 0
/// once Wi-Fi has run for 15 s; after [`CRASH_LIMIT`] failed attempts in a
/// row Wi-Fi is skipped, so a setting that crashes the board can still be
/// changed over USB. Crashes elsewhere do not count (a radio driver panic at
/// boot once disabled Wi-Fi for no reason). Cleared after [`STABLE_AFTER_S`]
/// seconds of running. The panic handler records where the last panic was.
/// Kept at 8 bytes: with 16 bytes or more of persistent RTC data the board
/// hung at boot (seen on a Heltec V4, cause not found). Packed as
/// `[marker (16) | file hash (16), line (16) | count (16)]` of the last panic.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut CRASHES: [u32; 2] = [0; 2];
const CRASH_MARK: u32 = 0x4D52_0000;
const CRASH_LIMIT: u32 = 2;
const STABLE_AFTER_S: u64 = 60;

fn crash_words() -> [u32; 2] {
    // SAFETY: single core use; read at start, written by the panic handler,
    // `wifi_start_task` and `stable_task`, never at the same time.
    let c = unsafe { core::ptr::addr_of!(CRASHES).read_volatile() };
    if c[0] & 0xFFFF_0000 == CRASH_MARK { c } else { [CRASH_MARK, 0] }
}

fn write_crash_words(c: [u32; 2]) {
    // SAFETY: see `crash_words`.
    unsafe { core::ptr::addr_of_mut!(CRASHES).write_volatile(c) };
}

fn crash_count() -> u32 {
    crash_words()[1] & 0xFFFF
}

fn set_crash_count(n: u32) {
    let c = crash_words();
    write_crash_words([c[0], (c[1] & 0xFFFF_0000) | n.min(0xFFFF)]);
}

fn fnv1a16(s: &str) -> u32 {
    let h = s.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
    (h ^ (h >> 16)) & 0xFFFF
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Restart instead of hanging: a repeater out in the field must come
    // back by itself.
    // Printed over USB (read with `espflash monitor`); a client reading
    // companion frames skips text it cannot parse.
    esp_println::println!("\r\nPANIC: {info}\r");
    let (line, file) = info.location().map(|l| (l.line().min(0xFFFF), fnv1a16(l.file()))).unwrap_or((0, 0));
    write_crash_words([CRASH_MARK | file, (line << 16) | crash_count()]);
    esp_hal::system::software_reset()
}


// ----- memory gauge -----
// The unused part of the main stack is filled with a pattern at boot; how
// much of it is still intact later is the stack that was never used.

unsafe extern "C" {
    static _stack_end: u32;
}
const STACK_PAINT: u32 = 0xA5A5_5A5A;
/// Leave the bottom alone: esp-hal keeps its stack guard word there.
const STACK_GUARD_SKIP: u32 = 256;

fn stack_bottom() -> u32 {
    core::ptr::addr_of!(_stack_end) as u32 + STACK_GUARD_SKIP
}

/// Fills the unused stack below the current stack pointer with the pattern.
#[inline(never)]
fn paint_stack() {
    let marker = 0u32;
    let sp = core::hint::black_box(&marker) as *const u32 as u32;
    let mut p = stack_bottom();
    while p + 1024 < sp {
        // SAFETY: between the stack bottom (plus the guard) and 1 KB below
        // the current stack pointer nothing is in use yet.
        unsafe { (p as *mut u32).write_volatile(STACK_PAINT) };
        p += 4;
    }
}

/// Stack never used since boot, in bytes.
pub fn stack_never_used() -> u32 {
    let start = stack_bottom();
    let mut p = start;
    // SAFETY: reads words inside the main stack region.
    while unsafe { (p as *const u32).read_volatile() } == STACK_PAINT {
        p += 4;
    }
    p - start
}

/// Updates the memory figures shown on the status page.
#[embassy_executor::task]
async fn memory_task() {
    use core::sync::atomic::Ordering::Relaxed;
    use maarse_fw_common::bus::{HEAP_FREE, HEAP_MIN_FREE, STACK_FREE};
    loop {
        STACK_FREE.store(stack_never_used(), Relaxed);
        let free = esp_alloc::HEAP.free() as u32;
        HEAP_FREE.store(free, Relaxed);
        let min = HEAP_MIN_FREE.load(Relaxed);
        if min == 0 || free < min {
            HEAP_MIN_FREE.store(free, Relaxed);
        }
        Timer::after_secs(2).await;
    }
}

/// Starts Wi-Fi some seconds after boot, so USB and Bluetooth are always
/// reachable first. The attempt is counted before it starts: if Wi-Fi crashes
/// the board, the count stays up, and
/// after [`CRASH_LIMIT`] attempts in a row Wi-Fi is skipped until the board
/// has run stably for [`STABLE_AFTER_S`] seconds.
#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn wifi_start_task(
    spawner: Spawner,
    wifi: esp_hal::peripherals::WIFI<'static>,
    settings: maarse_core::companion::WifiSettings,
    seed: u64,
) {
    Timer::after_secs(WIFI_DELAY_S).await;
    let n = crash_count();
    if n >= CRASH_LIMIT {
        maarse_fw_common::bus::WIFI_STATE.store(
            maarse_core::companion::wifi_state::SKIPPED_AFTER_CRASH as u32,
            core::sync::atomic::Ordering::Relaxed,
        );
        return;
    }
    set_crash_count(n + 1);
    wifi::start(&spawner, wifi, settings, seed);
    // Survived the start: ordinary restarts must not count as failed attempts.
    Timer::after_secs(15).await;
    set_crash_count(0);
}
const WIFI_DELAY_S: u64 = 10;


/// Clears the crash counter once the node has run long enough.
#[embassy_executor::task]
async fn stable_task() {
    Timer::after_secs(STABLE_AFTER_S).await;
    set_crash_count(0);
}

/// The Må:rse partition (see partitions.csv): settings, mailbox and
/// OTA meta page.
const MAARSE_PART: u32 = 0x210000;
const CFG_ADDR: u32 = MAARSE_PART;
const MAIL_START: u32 = MAARSE_PART + 0x1000;
/// Meta page for firmware updates (manifest + progress), see ota.rs.
const OTA_META: u32 = MAARSE_PART + 0x3000;
/// Known contacts (4 KB), kept across reboots.
const KNOWN_ADDR: u32 = MAARSE_PART + 0x4000;
/// Sensor recipe, owners and state (4 KB).
const SENSOR_ADDR: u32 = MAARSE_PART + 0x5000;

/// Restarts the board when the node asks (new Wi-Fi settings), after a
/// short pause so the reply reaches the client.
#[embassy_executor::task]
async fn restart_task() {
    maarse_fw_common::bus::RESTART.wait().await;
    Timer::after_millis(500).await;
    esp_hal::system::software_reset()
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    paint_stack();
    let p = esp_hal::init(esp_hal::Config::default());
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    // Heap for the BLE controller (esp-radio), 72 KB. The rest of the firmware
    // uses no heap. The 64 KB are in RAM that the bootloader frees
    // after start-up; otherwise the static data takes so much that the stack
    // has no room ("stack pointer out of range" and a restart at start-up).
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 8 * 1024);
    // Wi-Fi (when enabled) needs buffers on top of Bluetooth's. 72 KB more
    // left too little stack and the board restarted at boot; 40 KB leaves
    // about 65 KB of stack.

    // ----- board-specific pins -----
    // (sck, miso, mosi, cs, reset, dio1, busy, rf_rx, rf_tx, led, standard-radio)
    #[cfg(feature = "board-heltec-v4")]
    let (sck, miso, mosi, cs, reset, dio1, busy, rf_rx, rf_tx, led, default_radio) = {
        // PA module: V4.2 has GC1109, V4.3 has KCT8103L. Both get power via
        // GPIO7 and chip enable on GPIO2. Which one is fitted is seen (as in
        // Meshtastic) by reading GPIO2 as input after GPIO7 is switched on:
        // high = KCT8103L, low = GC1109.
        // During transmission the PA must be switched on via rf_tx, which the radio sets
        // high on TX and low otherwise:
        //   GC1109:   GPIO46 (CPS: high = full PA, low = bypass)
        //   KCT8103L: GPIO5  (CTX: high = TX, low = RX with LNA)
        let pa_power = Output::new(p.GPIO7, Level::High, OutputConfig::default());
        core::mem::forget(pa_power);
        Timer::after_millis(1).await;
        let mut gpio2 = p.GPIO2;
        let kct8103l = {
            let probe = Input::new(gpio2.reborrow(), InputConfig::default().with_pull(Pull::None));
            Timer::after_millis(1).await;
            probe.is_high()
        };
        let pa_en = Output::new(gpio2, Level::High, OutputConfig::default());
        core::mem::forget(pa_en);
        let pa_tx = if kct8103l {
            Output::new(p.GPIO5, Level::Low, OutputConfig::default())
        } else {
            Output::new(p.GPIO46, Level::Low, OutputConfig::default())
        };
        maarse_fw_common::bus::RADIO_DIAG[3].store(
            if kct8103l { 2 } else { 1 },
            core::sync::atomic::Ordering::Relaxed,
        );
        Timer::after_millis(5).await;
        (
            p.GPIO9,
            p.GPIO11,
            p.GPIO10,
            Output::new(p.GPIO8, Level::High, OutputConfig::default()),
            Output::new(p.GPIO12, Level::High, OutputConfig::default()),
            Input::new(p.GPIO14, InputConfig::default().with_pull(Pull::None)),
            Input::new(p.GPIO13, InputConfig::default().with_pull(Pull::None)),
            None::<Output<'static>>,
            Some(pa_tx),
            Output::new(p.GPIO35, Level::Low, OutputConfig::default()),
            // The PA gives about +7..+10 dB: 10 dBm in ≈ 17–20 dBm out.
            RadioConfig {
                tx_power_dbm: 10,
                ..RadioConfig::EU_DEFAULT
            },
        )
    };
    #[cfg(feature = "board-xiao-esp32s3")]
    let (sck, miso, mosi, cs, reset, dio1, busy, rf_rx, rf_tx, led, default_radio) = (
        // Wio-SX1262 on the XIAO ESP32-S3 (B2B connector).
        p.GPIO7,
        p.GPIO8,
        p.GPIO9,
        Output::new(p.GPIO41, Level::High, OutputConfig::default()),
        Output::new(p.GPIO42, Level::High, OutputConfig::default()),
        Input::new(p.GPIO39, InputConfig::default().with_pull(Pull::None)),
        Input::new(p.GPIO40, InputConfig::default().with_pull(Pull::None)),
        // RXEN on GPIO38: high during reception.
        Some(Output::new(p.GPIO38, Level::Low, OutputConfig::default())),
        None::<Output<'static>>,
        // The user LED (orange) on GPIO21, lit when low.
        Output::new(p.GPIO21, Level::High, OutputConfig::default()),
        RadioConfig::EU_DEFAULT,
    );
    // Only for code checking with the ordinary toolchain.
    #[cfg(feature = "check-esp32c6")]
    let (sck, miso, mosi, cs, reset, dio1, busy, rf_rx, rf_tx, led, default_radio) = (
        p.GPIO4,
        p.GPIO5,
        p.GPIO6,
        Output::new(p.GPIO7, Level::High, OutputConfig::default()),
        Output::new(p.GPIO2, Level::High, OutputConfig::default()),
        Input::new(p.GPIO3, InputConfig::default().with_pull(Pull::None)),
        Input::new(p.GPIO1, InputConfig::default().with_pull(Pull::None)),
        None::<Output<'static>>,
        None::<Output<'static>>,
        Output::new(p.GPIO8, Level::Low, OutputConfig::default()),
        RadioConfig::EU_DEFAULT,
    );
    let led_on_high = cfg!(not(feature = "board-xiao-esp32s3"));
    // Current profile for the range test's power estimates (0.1 mA). Typical
    // values, not measured: the ESP32-S3 running with Bluetooth is about 40 mA,
    // the Heltec V4's OLED about 10 mA more and its PA module adds an LNA on
    // receive and roughly 100 mA on transmit. See docs/FIELD-TEST.md.
    #[cfg(feature = "board-heltec-v4")]
    maarse_fw_common::power::set_profile(500, 95, 1000);
    #[cfg(not(feature = "board-heltec-v4"))]
    maarse_fw_common::power::set_profile(400, 55, 0);

    // ----- randomness and identity -----
    // True randomness from ADC noise, only used as a seed.
    #[allow(unused_mut)]
    let mut adc1 = p.ADC1;
    let trng_src = TrngSource::new(p.RNG, adc1.reborrow());
    let mut seed = [0u8; 32];
    match Trng::try_new() {
        Ok(mut t) => t.fill_bytes(&mut seed),
        Err(_) => esp_hal::rng::Rng::new().fill_bytes(&mut seed),
    }
    drop(trng_src);
    let mut rng = ChaCha20Rng::from_seed(seed);

    static MAIL_BUF: StaticCell<[u8; MAIL_AREA]> = StaticCell::new();
    let mut storage = FlashStorage::new(p.FLASH);
    let ota_cfg = ota::config(&mut storage, OTA_META);
    let mut flash = FlashSettings::new(storage, CFG_ADDR, MAIL_START, MAIL_BUF.init([0xFF; MAIL_AREA])).with_known(KNOWN_ADDR).with_sensor(SENSOR_ADDR);
    if let Some(c) = ota_cfg {
        flash = flash.with_ota(c);
    }
    let identity = flash.load_or_create_identity(&mut rng);
    let radio_cfg = flash.load_radio(default_radio);
    let wifi_settings = flash.load_wifi();
    // ----- sensor node: a timer wake out of deep sleep runs just one cycle -----
    // Bluetooth, Wi-Fi and the display stay off then. After a power-up or a
    // reset the node stays awake for a while (time to connect and set it up).
    let sensor_recipe = flash.load_sensor_recipe();
    let sensor_sleeps = maarse_core::sensor::recipe_sleeps(&sensor_recipe);
    let timer_wake = esp_hal::system::reset_reason() == Some(esp_hal::rtc_cntl::SocResetReason::CoreDeepSleep);
    let lean = timer_wake && sensor_sleeps;
    {
        use core::sync::atomic::Ordering::Relaxed;
        use maarse_fw_common::bus::{MONO_BASE_S, SENSOR_GRACE_S, SENSOR_PERSIST};
        SENSOR_PERSIST.store(true, Relaxed);
        SENSOR_GRACE_S.store(if sensor_sleeps && !timer_wake { 600 } else { 0 }, Relaxed);
        // A clock that goes on through deep sleep: the RTC timer.
        let rtc = esp_hal::rtc_cntl::Rtc::new(p.RTC_TIMER);
        MONO_BASE_S.store(rtc.time_since_power_up().as_secs() as u32, Relaxed);
    }
    let sensor_plan = maarse_core::sensor::recipe_pin_plan(&sensor_recipe);
    let wifi_on = wifi_settings.enabled && !lean;
    // Modem for a host over Wi-Fi: the node does not route (see wifi.rs).
    maarse_fw_common::bus::WIFI_MODEM.store(
        cfg!(feature = "wifi") && wifi_on && wifi_settings.modem,
        core::sync::atomic::Ordering::Relaxed,
    );

    // ----- LoRa -----
    let spi = Spi::new(
        p.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(8))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(sck)
    .with_miso(miso)
    .with_mosi(mosi)
    .into_async();
    let spi = ExclusiveDevice::new(spi, cs, Delay).unwrap();
    let iv = GenericSx126xInterfaceVariant::new(reset, dio1, busy, rf_rx, rf_tx).unwrap();
    let sx_cfg = sx126x::Config {
        chip: Sx1262,
        tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V8),
        use_dcdc: true,
        rx_boost: true,
    };
    let lora = LoRa::new(Sx126x::new(spi, iv, sx_cfg), false, Delay).await.ok();
    let radio_ok = lora.is_some();
    if let Some(lora) = lora {
        spawner.spawn(radio::radio_task(lora, led, led_on_high, rng.next_u64(), radio_cfg).unwrap());
    }

    // ----- USB (USB-Serial-JTAG) -----
    spawner.spawn(usb::usb_task(p.USB_DEVICE).unwrap());

    // ----- Bluetooth (Nordic UART Service) -----
    // Only one radio at a time: with both drivers built in (features `ble` and
    // `wifi`), the saved mode decides, and the heap then only has to hold one
    // of them (about 34 KB for Bluetooth, 37 KB for Wi-Fi), so no `coex`.
    #[cfg(feature = "ble")]
    if lean || cfg!(feature = "wifi") && wifi_on {
        let _ = p.BT;
    } else if let Ok(connector) = esp_radio::ble::controller::BleConnector::new(p.BT, Default::default()) {
        let addr = identity.address();
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&addr.0[..6]);
        mac[5] |= 0xC0; // random static address
        static NAME: StaticCell<heapless::String<16>> = StaticCell::new();
        let name = NAME.init({
            let mut n = heapless::String::new();
            let _ = n.push_str("Maarse-");
            for b in &addr.0[..2] {
                let _ = core::fmt::write(&mut n, format_args!("{b:02x}"));
            }
            n
        });
        let controller: ble::Ble = bt_hci::controller::ExternalController::new(connector);
        spawner.spawn(ble::ble_task(controller, mac, name.as_str()).unwrap());
    }

    // ----- Wi-Fi backbone (the `wifi` build; see wifi.rs) -----
    #[cfg(feature = "wifi")]
    maarse_fw_common::bus::WIFI_SUPPORTED.store(true, core::sync::atomic::Ordering::Relaxed);
    {
        use core::sync::atomic::Ordering::Relaxed;
        let c = crash_words();
        maarse_fw_common::bus::CRASHES.store(crash_count(), Relaxed);
        maarse_fw_common::bus::PANIC_LINE.store(c[1] >> 16, Relaxed);
        maarse_fw_common::bus::PANIC_FILE.store(c[0] & 0xFFFF, Relaxed);
    }
    #[cfg(feature = "wifi")]
    if wifi_on {
        spawner.spawn(wifi_start_task(spawner, p.WIFI, wifi_settings, rng.next_u64()).unwrap());
    }
    #[cfg(not(feature = "wifi"))]
    let _ = (&wifi_settings, p.WIFI);
    spawner.spawn(restart_task().unwrap());
    spawner.spawn(stable_task().unwrap());
    spawner.spawn(memory_task().unwrap());

    // ----- sensor pins (see sensor.rs) -----
    #[cfg(feature = "board-heltec-v4")]
    let (sensor_pins, gpio4_adc) = {
        use esp_hal::gpio::Flex;
        let i2c = if sensor_plan & 1 != 0 {
            let cfg = esp_hal::i2c::master::Config::default();
            esp_hal::i2c::master::I2c::new(p.I2C1, cfg).ok().map(|i| i.with_sda(p.GPIO47).with_scl(p.GPIO48).into_async())
        } else {
            None
        };
        let (gpio4_adc, gpio4_flex) = if sensor_plan & 2 != 0 { (Some(p.GPIO4), None) } else { (None, Some(Flex::new(p.GPIO4))) };
        let gpio = [
            Some(Flex::new(p.GPIO3)),
            gpio4_flex,
            Some(Flex::new(p.GPIO6)),
            Some(Flex::new(p.GPIO38)),
            Some(Flex::new(p.GPIO39)),
            Some(Flex::new(p.GPIO40)),
            Some(Flex::new(p.GPIO41)),
        ];
        // Woken from deep sleep by a wake pin (a reed switch): the node is told, so it
        // knows something happened instead of looking again at the next timer.
        for (i, f) in gpio.iter().enumerate() {
            if let Some(f) = f
                && f.caused_wakeup()
            {
                maarse_fw_common::bus::BOOT_WAKE_PIN.store(sensor::DIGITAL[i] as i32, core::sync::atomic::Ordering::Relaxed);
            }
        }
        (sensor::Pins { gpio, i2c }, gpio4_adc)
    };
    #[cfg(not(feature = "board-heltec-v4"))]
    let sensor_pins = {
        let _ = sensor_plan;
        sensor::Pins { gpio: [const { None }; 7], i2c: None }
    };
    spawner.spawn(sensor::sensor_task(sensor_pins).unwrap());
    spawner.spawn(sensor::sleep_task(p.LPWR).unwrap());

    // ----- battery (Heltec V4; the XIAO ESP32-S3 has no divider: unknown) -----
    #[cfg(feature = "board-heltec-v4")]
    {
        let (adc, pin, pin4) = battery::setup(adc1, p.GPIO1, gpio4_adc);
        let ctrl = Output::new(p.GPIO37, Level::Low, OutputConfig::default());
        spawner.spawn(battery::battery_task(adc, pin, ctrl, pin4).unwrap());
    }
    #[cfg(not(feature = "board-heltec-v4"))]
    let _ = adc1;

    // ----- display (Heltec V4) -----
    #[cfg(feature = "board-heltec-v4")]
    {
        // Vext (GPIO36, active low) powers the OLED; reset on GPIO21.
        let vext = Output::new(p.GPIO36, Level::Low, OutputConfig::default());
        core::mem::forget(vext);
        // A timer wake of a sensor node runs without the display.
        if !lean {
            let mut rst = Output::new(p.GPIO21, Level::Low, OutputConfig::default());
            Timer::after_millis(20).await;
            rst.set_high();
            core::mem::forget(rst);
            Timer::after_millis(20).await;
            let i2c = esp_hal::i2c::master::I2c::new(p.I2C0, esp_hal::i2c::master::Config::default())
                .unwrap()
                .with_sda(p.GPIO17)
                .with_scl(p.GPIO18);
            let button = Input::new(p.GPIO0, InputConfig::default().with_pull(Pull::Up));
            spawner.spawn(display::display_task(i2c, button).unwrap());
        }
    }

    // ----- the node itself -----
    // The node (about 65 KB) runs in its own task. Kept in `main`, it made the
    // main task's future 67 KB, and the stack frame that builds that future
    // stays reserved for the whole run: 95 KB of stack were gone before
    // `esp_hal::init` even returned.
    spawner.spawn(node_task(identity, rng, flash, radio_cfg, radio_ok, wifi_on).unwrap());
}

type Node = maarse_fw_common::node::FwNode<ChaCha20Rng, FlashSettings<FlashStorage<'static>>>;

#[embassy_executor::task]
async fn node_task(
    identity: maarse_crypto::Identity,
    rng: ChaCha20Rng,
    flash: FlashSettings<FlashStorage<'static>>,
    radio_cfg: RadioConfig,
    radio_ok: bool,
    wifi_on: bool,
) {
    static NODE: StaticCell<Node> = StaticCell::new();
    // Built in place: built on the stack and copied, it left 4.5 KB of stack.
    let node = Node::new_in(NODE.uninit(), identity, rng, flash, radio_cfg, radio_ok);
    esp_println::println!(
        "\r\nboot: node ready; stack never used {} B, heap free {} B, wifi {}\r",
        stack_never_used(),
        esp_alloc::HEAP.free(),
        if !cfg!(feature = "wifi") { "not in this build" } else if wifi_on { "on" } else { "off" }
    );
    node.run().await;
}
