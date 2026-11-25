//! Må:rse firmware for nRF52840 + SX1262 (RAK WisMesh Tag, XIAO nRF52840 + Wio-SX1262).
//!
//! * LoRa: default 869.525 MHz, 125 kHz, SF9, CR 4/5, 22 dBm, 10 % duty
//!   cycle. Can be changed from the web client and is stored in flash.
//! * USB (serial): the companion protocol for the web client, or
//!   modem mode when a maarse-host uses the node as a radio.
//! * Bluetooth (Nordic UART Service): the same companion protocol as USB.
//!
//! Build: see scripts/build-firmware.sh (or .ps1).

#![no_std]
#![no_main]

mod battery;
mod ble;
mod ota;
mod radio;
mod sensor;
mod storage;
#[cfg(feature = "board-wismesh-tag")]
mod tag;
mod usb;

use embassy_executor::Spawner;
use maarse_fw_common::node::Settings as _;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::rng::{self, Rng};
use nrf_sdc::mpsl::MultiprotocolServiceLayer;
use nrf_sdc::{self as sdc, mpsl};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use static_cell::StaticCell;
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::interrupt::{self, InterruptExt, Priority};
use embassy_nrf::{bind_interrupts, peripherals};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use lora_phy::LoRa;
use lora_phy::iv::GenericSx126xInterfaceVariant;
use lora_phy::sx126x::{self, Sx126x, Sx1262, TcxoCtrlVoltage};
use panic_halt as _;
use rand_core::RngCore;

bind_interrupts!(pub struct Irqs {
    SPI2 => spim::InterruptHandler<peripherals::SPI2>;
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    RNG => rng::InterruptHandler<peripherals::RNG>;
    SAADC => embassy_nrf::saadc::InterruptHandler;
    TWISPI0 => embassy_nrf::twim::InterruptHandler<peripherals::TWISPI0>;
    #[cfg(feature = "board-wismesh-tag")]
    UARTE1 => embassy_nrf::uarte::InterruptHandler<peripherals::UARTE1>;
    EGU0_SWI0 => nrf_sdc::mpsl::LowPrioInterruptHandler;
    CLOCK_POWER => nrf_sdc::mpsl::ClockInterruptHandler;
    RADIO => nrf_sdc::mpsl::HighPrioInterruptHandler;
    TIMER0 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    RTC0 => nrf_sdc::mpsl::HighPrioInterruptHandler;
});

#[embassy_executor::task]
async fn mpsl_task(mpsl: &'static MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

/// The chip temperature every minute. The TEMP peripheral is owned by MPSL, so it is read
/// through it (unit 0.25 °C).
#[embassy_executor::task]
async fn temp_task(mpsl: &'static MultiprotocolServiceLayer<'static>) -> ! {
    loop {
        let raw = mpsl.get_temperature().raw();
        maarse_fw_common::power::TEMP_DC.store(raw * 10 / 4, core::sync::atomic::Ordering::Relaxed);
        embassy_time::Timer::after_secs(60).await;
    }
}

/// Restarts when the node asks (a new sensor recipe needs other pins).
#[embassy_executor::task]
async fn restart_task() {
    maarse_fw_common::bus::RESTART.wait().await;
    embassy_time::Timer::after_millis(500).await;
    cortex_m::peripheral::SCB::sys_reset();
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // The clocks are controlled by MPSL (Bluetooth); the external crystal for USB is requested
    // further down.
    // MPSL reserves interrupt priorities 0, 1 and 4. Everything else must be
    // lower (2, 3, 5-7), otherwise e.g. the DIO1 interrupt from LoRa can be lost.
    let mut nrf_cfg = embassy_nrf::config::Config::default();
    nrf_cfg.gpiote_interrupt_priority = Priority::P2;
    nrf_cfg.time_interrupt_priority = Priority::P2;
    // The tag has the 32.768 kHz crystal (MeshCore: USE_LFXO) and the DC/DC inductor: the
    // crystal is more exact and costs less than the RC oscillator, the DC/DC saves current.
    #[cfg(feature = "board-wismesh-tag")]
    {
        nrf_cfg.lfclk_source = embassy_nrf::config::LfclkSource::ExternalXtal;
        nrf_cfg.dcdc.reg1 = true;
    }
    let p = embassy_nrf::init(nrf_cfg);
    interrupt::SPI2.set_priority(Priority::P3);
    interrupt::USBD.set_priority(Priority::P3);
    interrupt::RNG.set_priority(Priority::P3);
    interrupt::SAADC.set_priority(Priority::P3);
    interrupt::TWISPI0.set_priority(Priority::P3);
    #[cfg(feature = "board-wismesh-tag")]
    interrupt::UARTE1.set_priority(Priority::P3);

    // ----- board-specific pins -----
    #[cfg(feature = "board-wismesh-tag")]
    let (sck, miso, mosi, cs, reset, dio1, busy, rf_rx, led) = {
        // RAK4630: the SX1262 gets power via P1.05; TX LED (green) on P1.03. The blue LED on
        // P1.04 is lit until `tag::ui_task` takes over, so a hang while starting shows.
        let power = Output::new(p.P1_05, Level::High, OutputDrive::Standard);
        core::mem::forget(power); // Must stay on.
        embassy_time::Timer::after_millis(10).await;
        (
            p.P1_11,
            p.P1_13,
            p.P1_12,
            Output::new(p.P1_10, Level::High, OutputDrive::Standard),
            Output::new(p.P1_06, Level::High, OutputDrive::Standard),
            Input::new(p.P1_15, Pull::None),
            Input::new(p.P1_14, Pull::None),
            None::<Output<'static>>,
            (
                Output::new(p.P1_03, Level::Low, OutputDrive::Standard),
                true,
            ),
        )
    };
    #[cfg(feature = "board-wismesh-tag")]
    let blue = Output::new(p.P1_04, Level::High, OutputDrive::Standard);
    #[cfg(feature = "board-xiao-nrf52840")]
    let (sck, miso, mosi, cs, reset, dio1, busy, rf_rx, led) = (
        // Wio-SX1262 for the XIAO: CS D4, DIO1 D1, BUSY D3, RESET D2, RXEN D5.
        p.P1_13,
        p.P1_14,
        p.P1_15,
        Output::new(p.P0_04, Level::High, OutputDrive::Standard),
        Output::new(p.P0_28, Level::High, OutputDrive::Standard),
        Input::new(p.P0_03, Pull::None),
        Input::new(p.P0_29, Pull::None),
        Some(Output::new(p.P0_05, Level::Low, OutputDrive::Standard)),
        // Green LED on P0.30, lit when low.
        (
            Output::new(p.P0_30, Level::High, OutputDrive::Standard),
            false,
        ),
    );

    // ----- current profile for the range test's power estimates -----
    // Typical values, not measured on this board: MCU with USB and Bluetooth
    // advertising about 3 mA, SX1262 receiving (boosted) 5.3 mA plus the RF
    // switch; no external PA. See docs/FIELD-TEST.md for how to measure them.
    maarse_fw_common::power::set_profile(30, 55, 0);
    // ----- battery (see battery.rs) -----
    {
        use embassy_nrf::saadc::{ChannelConfig, Config, Gain, Saadc, Time};
        #[cfg(feature = "board-xiao-nrf52840")]
        let (pin, enable) = (
            p.P0_31,
            // VBAT_ENABLE (P0.14) low = the divider is on.
            Some(Output::new(p.P0_14, Level::Low, OutputDrive::Standard)),
        );
        #[cfg(feature = "board-wismesh-tag")]
        let (pin, enable) = (p.P0_05, None::<Output<'static>>);
        let mut ch = ChannelConfig::single_ended(pin);
        ch.gain = Gain::Gain1_5; // 0.6 V / (1/5) = 3.0 V full scale
        ch.time = Time::_40US; // high source impedance (MΩ divider)
        let adc = Saadc::new(p.SAADC, Irqs, Config::default(), [ch]);
        spawner.spawn(battery::battery_task(adc, enable).unwrap());
    }

    // ----- randomness and identity -----
    // Hardware randomness: used by the Bluetooth controller and as a seed for
    // a ChaCha20 generator for the node's cryptography.
    static RNG: StaticCell<Rng<'static, embassy_nrf::mode::Async>> = StaticCell::new();
    let hw_rng = RNG.init(Rng::new(p.RNG, Irqs));
    let mut seed = [0u8; 32];
    hw_rng.blocking_fill_bytes(&mut seed);
    let mut rng = ChaCha20Rng::from_seed(seed);
    static MAIL_BUF: StaticCell<[u8; maarse_fw_common::storage::MAIL_AREA]> = StaticCell::new();
    let mut flash = storage::Flash::new(
        Nvmc::new(p.NVMC),
        storage::CFG_ADDR,
        storage::MAIL_START,
        MAIL_BUF.init([0xFF; maarse_fw_common::storage::MAIL_AREA]),
    )
    .with_ota(ota::config())
    .with_known(storage::KNOWN_ADDR)
    .with_sensor(storage::SENSOR_ADDR);
    let identity = flash.load_or_create_identity(&mut rng);
    let radio_cfg = flash.load_radio(maarse_core::companion::RadioConfig::EU_DEFAULT);
    let addr = identity.address();

    // ----- sensor node: what the stored recipe needs, and whether to sleep -----
    // On battery with a recipe that sleeps, Bluetooth, USB and the 32 MHz
    // crystal stay off between cycles (plug in USB and press reset to talk to
    // the node over USB).
    let sensor_recipe = flash.load_sensor_recipe();
    let usb_power = battery::vbus_present();
    maarse_fw_common::power::EXT_POWER.store(usb_power as i32, core::sync::atomic::Ordering::Relaxed);
    let low_power = maarse_core::sensor::recipe_sleeps(&sensor_recipe) && !usb_power;
    let sensor_plan = maarse_core::sensor::recipe_pin_plan(&sensor_recipe);
    #[cfg(feature = "board-xiao-nrf52840")]
    let sensor_pins = {
        use embassy_nrf::gpio::Flex;
        let d0 = Flex::new(p.P0_02);
        let (d6, d7, i2c) = if sensor_plan & 1 != 0 {
            // I2C for a BME280: D6 = SDA, D7 = SCL.
            let mut cfg = embassy_nrf::twim::Config::default();
            cfg.sda_pullup = true;
            cfg.scl_pullup = true;
            static TX_RAM: StaticCell<[u8; 16]> = StaticCell::new();
            let twim = embassy_nrf::twim::Twim::new(p.TWISPI0, Irqs, p.P1_11, p.P1_12, cfg, TX_RAM.init([0; 16]));
            (None, None, Some(twim))
        } else {
            (Some(Flex::new(p.P1_11)), Some(Flex::new(p.P1_12)), None)
        };
        sensor::Pins { d0, d6, d7, i2c }
    };
    #[cfg(feature = "board-wismesh-tag")]
    let sensor_pins = {
        // I2C on the module's header: SDA P0.25, SCL P0.24 (as in MeshCore's variant).
        let i2c = (sensor_plan & 1 != 0).then(|| {
            let mut cfg = embassy_nrf::twim::Config::default();
            cfg.sda_pullup = true;
            cfg.scl_pullup = true;
            static TX_RAM: StaticCell<[u8; 16]> = StaticCell::new();
            embassy_nrf::twim::Twim::new(p.TWISPI0, Irqs, p.P0_25, p.P0_24, cfg, TX_RAM.init([0; 16]))
        });
        sensor::Pins { i2c }
    };

    // ----- LoRa -----
    let mut spi_cfg = spim::Config::default();
    spi_cfg.frequency = spim::Frequency::M1;
    let spim = Spim::new(p.SPI2, Irqs, sck, miso, mosi, spi_cfg);
    let spi = ExclusiveDevice::new(spim, cs, Delay).unwrap();
    let (mut spi, mut reset, busy) = (spi, reset, busy);
    radio_selftest(&mut spi, &mut reset, &busy).await;
    let iv = GenericSx126xInterfaceVariant::new(reset, dio1, busy, rf_rx, None).unwrap();
    let sx_cfg = sx126x::Config {
        chip: Sx1262,
        tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V8),
        use_dcdc: true,
        rx_boost: true,
    };
    // lora-phy waits for the BUSY pin without a limit, so a radio that never answers (wrong
    // pins, no power) would hang the start-up before USB and Bluetooth exist. The timeout
    // keeps the node reachable without a radio.
    let lora = match embassy_time::with_timeout(
        embassy_time::Duration::from_secs(3),
        LoRa::new(Sx126x::new(spi, iv, sx_cfg), false, Delay),
    )
    .await
    {
        Ok(Ok(l)) => Some(l),
        // The radio does not answer: the node carries on without it, so USB works.
        Ok(Err(_)) | Err(_) => None,
    };
    let radio_ok = lora.is_some();
    #[cfg(feature = "board-wismesh-tag")]
    tag::RADIO_FAILED.store(!radio_ok, core::sync::atomic::Ordering::Relaxed);
    if let Some(lora) = lora {
        spawner.spawn(radio::radio_task(lora, led.0, led.1, rng.next_u64(), radio_cfg).unwrap());
    }

    // ----- Bluetooth (MPSL + SoftDevice Controller) -----
    let mpsl_p = mpsl::Peripherals::new(p.RTC0, p.TIMER0, p.TEMP, p.PPI_CH19, p.PPI_CH30, p.PPI_CH31);
    let lfclk = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        #[cfg(feature = "board-wismesh-tag")]
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_XTAL as u8,
        #[cfg(not(feature = "board-wismesh-tag"))]
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_RC as u8,
        // The crystal needs no calibration of an RC oscillator.
        #[cfg(feature = "board-wismesh-tag")]
        rc_ctiv: 0,
        #[cfg(not(feature = "board-wismesh-tag"))]
        rc_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_CTIV as u8,
        #[cfg(feature = "board-wismesh-tag")]
        rc_temp_ctiv: 0,
        #[cfg(not(feature = "board-wismesh-tag"))]
        rc_temp_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_TEMP_CTIV as u8,
        #[cfg(feature = "board-wismesh-tag")]
        accuracy_ppm: 20,
        #[cfg(not(feature = "board-wismesh-tag"))]
        accuracy_ppm: mpsl::raw::MPSL_DEFAULT_CLOCK_ACCURACY_PPM as u16,
        skip_wait_lfclk_started: mpsl::raw::MPSL_DEFAULT_SKIP_WAIT_LFCLK_STARTED != 0,
    };
    static MPSL: StaticCell<MultiprotocolServiceLayer> = StaticCell::new();
    let mpsl = MPSL.init(mpsl::MultiprotocolServiceLayer::new(mpsl_p, Irqs, lfclk).unwrap());
    spawner.spawn(mpsl_task(mpsl).unwrap());
    spawner.spawn(temp_task(mpsl).unwrap());
    // USB needs the external crystal all the time.
    if !low_power && let Ok(hf) = mpsl.request_hfclk().await {
        core::mem::forget(hf);
    }
    let sdc_p = sdc::Peripherals::new(
        p.PPI_CH17, p.PPI_CH18, p.PPI_CH20, p.PPI_CH21, p.PPI_CH22, p.PPI_CH23, p.PPI_CH24, p.PPI_CH25,
        p.PPI_CH26, p.PPI_CH27, p.PPI_CH28, p.PPI_CH29,
    );
    static SDC_MEM: StaticCell<sdc::Mem<4720>> = StaticCell::new();
    let sdc_mem = SDC_MEM.init(sdc::Mem::new());
    let built = sdc::Builder::new()
        .map(|b| b.support_adv().support_peripheral())
        .and_then(|b| b.peripheral_count(1))
        .and_then(|b| b.buffer_cfg(251, 251, 3, 3))
        .and_then(|b| b.build(sdc_p, hw_rng, mpsl, sdc_mem));
    if let (Ok(controller), false) = (built, low_power) {
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
        spawner.spawn(ble::ble_task(controller, mac, name.as_str()).unwrap());
    }

    // ----- USB -----
    if !low_power {
        spawner.spawn(usb::usb_task(p.USBD, Irqs).unwrap());
    }

    // ----- WisMesh Tag: button, LEDs, buzzer and GPS -----
    #[cfg(feature = "board-wismesh-tag")]
    {
        use embassy_nrf::gpio::Pull as P;
        use embassy_nrf::uarte::{self, Baudrate, Uarte};
        let button = Input::new(p.P0_09, P::Up);
        let buzzer = Output::new(p.P0_21, Level::Low, OutputDrive::Standard); // idle low
        spawner.spawn(tag::ui_task(blue, buzzer, button).unwrap());
        let mut ucfg = uarte::Config::default();
        ucfg.baudrate = Baudrate::Baud9600;
        let (_tx, rx) = Uarte::new(p.UARTE1, p.P0_15, p.P0_16, Irqs, ucfg).split_with_idle(p.TIMER1, p.PPI_CH0, p.PPI_CH1);
        core::mem::forget(_tx);
        let gps_en = Output::new(p.P1_02, Level::Low, OutputDrive::Standard);
        spawner.spawn(tag::gps_task(gps_en, rx).unwrap());
    }

    // ----- sensors, and restarts when a new recipe needs other pins -----
    let wake_pins = maarse_core::sensor::Recipe::from_bytes(&sensor_recipe).map(|r| r.wake_pins()).unwrap_or_default();
    spawner.spawn(sensor::sensor_task(sensor_pins, wake_pins).unwrap());
    spawner.spawn(restart_task().unwrap());

    // ----- the node itself -----
    let mut node =
        maarse_fw_common::node::FwNode::new(identity, rng, flash, radio_cfg, radio_ok);
    node.run().await;
}

/// Talks directly to the SX1262 over SPI before lora-phy takes over: reads the sync
/// word, status and error flags after TCXO start and calibration. The result
/// can be seen in the status (keys 55-58).
async fn radio_selftest(
    spi: &mut ExclusiveDevice<Spim<'static>, Output<'static>, Delay>,
    reset: &mut Output<'static>,
    busy: &Input<'static>,
) {
    use embedded_hal_async::spi::SpiDevice;
    use maarse_fw_common::bus::RADIO_DIAG;
    use core::sync::atomic::Ordering::Relaxed;
    let wait_busy = async || {
        for _ in 0..200 {
            if busy.is_low() {
                return;
            }
            embassy_time::Timer::after_millis(1).await;
        }
    };
    reset.set_low();
    embassy_time::Timer::after_millis(20).await;
    reset.set_high();
    embassy_time::Timer::after_millis(10).await;
    wait_busy().await;
    RADIO_DIAG[3].store(busy.is_high() as u32, Relaxed);
    // ReadRegister 0x0740 (2 bytes): opcode, address, NOP, data, data.
    let mut b = [0x1D, 0x07, 0x40, 0, 0, 0];
    let _ = spi.transfer_in_place(&mut b).await;
    RADIO_DIAG[0].store(u32::from(b[4]) << 8 | u32::from(b[5]), Relaxed);
    wait_busy().await;
    // GetStatus.
    let mut b = [0xC0, 0];
    let _ = spi.transfer_in_place(&mut b).await;
    RADIO_DIAG[1].store(b[1] as u32, Relaxed);
    wait_busy().await;
    // SetDIO3AsTCXOCtrl 1.8 V, timeout 5 ms (320 × 15.625 µs).
    let _ = spi.write(&[0x97, 0x02, 0x00, 0x01, 0x40]).await;
    wait_busy().await;
    // ClearDeviceErrors, Calibrate(all), wait.
    let _ = spi.write(&[0x07, 0x00, 0x00]).await;
    wait_busy().await;
    let _ = spi.write(&[0x89, 0x7F]).await;
    embassy_time::Timer::after_millis(10).await;
    wait_busy().await;
    // GetDeviceErrors: status, errors (2 bytes).
    let mut b = [0x17, 0, 0, 0];
    let _ = spi.transfer_in_place(&mut b).await;
    RADIO_DIAG[2].store(u32::from(b[2]) << 8 | u32::from(b[3]), Relaxed);
}
