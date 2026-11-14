//! Må:rse-firmware til ESP32-S3 + SX1262.
//!
//! * Heltec WiFi LoRa 32 V4 (med OLED-display og PA-forstærker)
//! * Seeed XIAO ESP32-S3 + Wio-SX1262
//!
//! Samme opførsel som nRF-firmwaren: companion-protokollen over USB
//! (USB-Serial-JTAG), modem-tilstand for maarse-host, radioindstillinger og
//! administratorer i flash, en postkasse i flash og Bluetooth (Nordic UART
//! Service, samme GATT-tjeneste og navn som på nRF).
//!
//! Byg: scripts/build-firmware.sh heltec-v4 (kræver espup og espflash).

#![no_std]
#![no_main]

#[cfg(feature = "board-heltec-v4")]
mod battery;
mod ble;
#[cfg(feature = "display")]
mod display;
mod ota;
mod radio;
mod usb;

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
use maarse_fw_common::storage::{FlashSettings, MAIL_AREA};

esp_bootloader_esp_idf::esp_app_desc!();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // Genstart i stedet for at hænge: en repeater ude i marken skal komme
    // tilbage af sig selv.
    esp_hal::system::software_reset()
}

/// Må:rse-partitionen (se partitions.csv): indstillinger, postkasse og
/// OTA-metaside.
const MAARSE_PART: u32 = 0x210000;
const CFG_ADDR: u32 = MAARSE_PART;
const MAIL_START: u32 = MAARSE_PART + 0x1000;
/// Metaside til firmwareopdatering (manifest + fremskridt), se ota.rs.
const OTA_META: u32 = MAARSE_PART + 0x3000;

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let p = esp_hal::init(esp_hal::Config::default());
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    // Heap til BLE-controlleren (esp-radio), 72 KB. Resten af firmwaren
    // bruger ingen heap. De 64 KB ligger i RAM, som bootloaderen frigiver
    // efter opstart; ellers fylder de statiske data så meget, at stakken
    // ikke er plads ("stack pointer out of range" og genstart ved opstart).
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 8 * 1024);

    // ----- board-specifikke ben -----
    // (sck, miso, mosi, cs, reset, dio1, busy, rf_rx, rf_tx, led, standard-radio)
    #[cfg(feature = "board-heltec-v4")]
    let (sck, miso, mosi, cs, reset, dio1, busy, rf_rx, rf_tx, led, default_radio) = {
        // PA-modul: V4.2 har GC1109, V4.3 har KCT8103L. Begge får strøm via
        // GPIO7 og chip-enable på GPIO2. Hvilken der sidder, ses (som i
        // Meshtastic) ved at læse GPIO2 som input, efter GPIO7 er tændt:
        // høj = KCT8103L, lav = GC1109.
        // Under afsendelse skal PA'en slås til via rf_tx, som radioen sætter
        // høj ved TX og lav ellers:
        //   GC1109:   GPIO46 (CPS: høj = fuld PA, lav = bypass)
        //   KCT8103L: GPIO5  (CTX: høj = TX, lav = RX med LNA)
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
            // PA'en giver ca. +7..+10 dB: 10 dBm ind ≈ 17–20 dBm ud.
            RadioConfig {
                tx_power_dbm: 10,
                ..RadioConfig::EU_DEFAULT
            },
        )
    };
    #[cfg(feature = "board-xiao-esp32s3")]
    let (sck, miso, mosi, cs, reset, dio1, busy, rf_rx, rf_tx, led, default_radio) = (
        // Wio-SX1262 på XIAO ESP32-S3 (B2B-stik).
        p.GPIO7,
        p.GPIO8,
        p.GPIO9,
        Output::new(p.GPIO41, Level::High, OutputConfig::default()),
        Output::new(p.GPIO42, Level::High, OutputConfig::default()),
        Input::new(p.GPIO39, InputConfig::default().with_pull(Pull::None)),
        Input::new(p.GPIO40, InputConfig::default().with_pull(Pull::None)),
        // RXEN på GPIO38: høj under modtagelse.
        Some(Output::new(p.GPIO38, Level::Low, OutputConfig::default())),
        None::<Output<'static>>,
        // Brugerens LED (orange) på GPIO21, tænder ved lav.
        Output::new(p.GPIO21, Level::High, OutputConfig::default()),
        RadioConfig::EU_DEFAULT,
    );
    // Kun til kodetjek med den almindelige værktøjskæde.
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

    // ----- tilfældighed og identitet -----
    // Ægte tilfældighed fra ADC-støj, kun brugt som frø.
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
    let mut flash = FlashSettings::new(storage, CFG_ADDR, MAIL_START, MAIL_BUF.init([0xFF; MAIL_AREA]));
    if let Some(c) = ota_cfg {
        flash = flash.with_ota(c);
    }
    let identity = flash.load_or_create_identity(&mut rng);
    let radio_cfg = flash.load_radio(default_radio);

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
    if let Ok(connector) = esp_radio::ble::controller::BleConnector::new(p.BT, Default::default()) {
        let addr = identity.address();
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&addr.0[..6]);
        mac[5] |= 0xC0; // tilfældig statisk adresse
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

    // ----- batteri (Heltec V4; XIAO ESP32-S3 har ingen deler: ukendt) -----
    #[cfg(feature = "board-heltec-v4")]
    {
        let (adc, pin) = battery::setup(adc1, p.GPIO1);
        let ctrl = Output::new(p.GPIO37, Level::Low, OutputConfig::default());
        spawner.spawn(battery::battery_task(adc, pin, ctrl).unwrap());
    }
    #[cfg(not(feature = "board-heltec-v4"))]
    let _ = adc1;

    // ----- display (Heltec V4) -----
    #[cfg(feature = "board-heltec-v4")]
    {
        // Vext (GPIO36, aktiv lav) forsyner OLED'en; reset på GPIO21.
        let vext = Output::new(p.GPIO36, Level::Low, OutputConfig::default());
        core::mem::forget(vext);
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

    // ----- noden selv -----
    let mut node = maarse_fw_common::node::FwNode::new(identity, rng, flash, radio_cfg, radio_ok);
    node.run().await;
}
