//! Må:rse-firmware til nRF52840 + SX1262 (RAK WisMesh Tag, XIAO nRF52840 + Wio-SX1262).
//!
//! * LoRa: standard 869,525 MHz, 125 kHz, SF9, CR 4/5, 22 dBm, 10 % duty
//!   cycle. Kan ændres fra webklienten og gemmes i flash.
//! * USB (seriel): companion-protokollen til webklienten, eller
//!   modem-tilstand, når en maarse-host bruger noden som radio.
//! * Bluetooth (Nordic UART Service): samme companion-protokol som USB.
//!
//! Byg: se scripts/build-firmware.sh (eller .ps1).

#![no_std]
#![no_main]

mod battery;
mod ble;
mod ota;
mod radio;
mod storage;
mod usb;

use embassy_executor::Spawner;
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

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // Urene styres af MPSL (Bluetooth); den eksterne krystal til USB bedes
    // om længere nede.
    // MPSL reserverer interrupt-prioritet 0, 1 og 4. Alt andet skal ligge
    // lavere (2, 3, 5-7), ellers kan fx DIO1-interruptet fra LoRa gå tabt.
    let mut nrf_cfg = embassy_nrf::config::Config::default();
    nrf_cfg.gpiote_interrupt_priority = Priority::P2;
    nrf_cfg.time_interrupt_priority = Priority::P2;
    let p = embassy_nrf::init(nrf_cfg);
    interrupt::SPI2.set_priority(Priority::P3);
    interrupt::USBD.set_priority(Priority::P3);
    interrupt::RNG.set_priority(Priority::P3);
    interrupt::SAADC.set_priority(Priority::P3);

    // ----- board-specifikke ben -----
    #[cfg(feature = "board-wismesh-tag")]
    let (sck, miso, mosi, cs, reset, dio1, busy, rf_rx, led) = {
        // RAK4630: SX1262 får strøm via P1.05; LED (blå) på P1.04.
        let power = Output::new(p.P1_05, Level::High, OutputDrive::Standard);
        core::mem::forget(power); // Skal blive tændt.
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
                Output::new(p.P1_04, Level::Low, OutputDrive::Standard),
                true,
            ),
        )
    };
    #[cfg(feature = "board-xiao-nrf52840")]
    let (sck, miso, mosi, cs, reset, dio1, busy, rf_rx, led) = (
        // Wio-SX1262 til XIAO: CS D4, DIO1 D1, BUSY D3, RESET D2, RXEN D5.
        p.P1_13,
        p.P1_14,
        p.P1_15,
        Output::new(p.P0_04, Level::High, OutputDrive::Standard),
        Output::new(p.P0_28, Level::High, OutputDrive::Standard),
        Input::new(p.P0_03, Pull::None),
        Input::new(p.P0_29, Pull::None),
        Some(Output::new(p.P0_05, Level::Low, OutputDrive::Standard)),
        // Grøn LED på P0.30, tænder ved lav.
        (
            Output::new(p.P0_30, Level::High, OutputDrive::Standard),
            false,
        ),
    );

    // ----- batteri (se battery.rs) -----
    {
        use embassy_nrf::saadc::{ChannelConfig, Config, Gain, Saadc, Time};
        #[cfg(feature = "board-xiao-nrf52840")]
        let (pin, enable) = (
            p.P0_31,
            // VBAT_ENABLE (P0.14) lav = deleren tændt.
            Some(Output::new(p.P0_14, Level::Low, OutputDrive::Standard)),
        );
        #[cfg(feature = "board-wismesh-tag")]
        let (pin, enable) = (p.P0_05, None::<Output<'static>>);
        let mut ch = ChannelConfig::single_ended(pin);
        ch.gain = Gain::Gain1_5; // 0,6 V / (1/5) = 3,0 V fuldt udslag
        ch.time = Time::_40US; // høj kildeimpedans (MΩ-deler)
        let adc = Saadc::new(p.SAADC, Irqs, Config::default(), [ch]);
        spawner.spawn(battery::battery_task(adc, enable).unwrap());
    }

    // ----- tilfældighed og identitet -----
    // Hardware-tilfældighed: bruges af Bluetooth-controlleren og som frø til
    // en ChaCha20-generator for nodens kryptografi.
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
    .with_ota(ota::config());
    let identity = flash.load_or_create_identity(&mut rng);
    let radio_cfg = flash.load_radio(maarse_core::companion::RadioConfig::EU_DEFAULT);
    let addr = identity.address();

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
    let lora = match LoRa::new(Sx126x::new(spi, iv, sx_cfg), false, Delay).await {
        Ok(l) => Some(l),
        Err(_) => None, // Radioen svarer ikke: noden kører videre uden, så USB virker.
    };
    let radio_ok = lora.is_some();
    if let Some(lora) = lora {
        spawner.spawn(radio::radio_task(lora, led.0, led.1, rng.next_u64(), radio_cfg).unwrap());
    }

    // ----- Bluetooth (MPSL + SoftDevice Controller) -----
    let mpsl_p = mpsl::Peripherals::new(p.RTC0, p.TIMER0, p.TEMP, p.PPI_CH19, p.PPI_CH30, p.PPI_CH31);
    let lfclk = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_RC as u8,
        rc_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_CTIV as u8,
        rc_temp_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_TEMP_CTIV as u8,
        accuracy_ppm: mpsl::raw::MPSL_DEFAULT_CLOCK_ACCURACY_PPM as u16,
        skip_wait_lfclk_started: mpsl::raw::MPSL_DEFAULT_SKIP_WAIT_LFCLK_STARTED != 0,
    };
    static MPSL: StaticCell<MultiprotocolServiceLayer> = StaticCell::new();
    let mpsl = MPSL.init(mpsl::MultiprotocolServiceLayer::new(mpsl_p, Irqs, lfclk).unwrap());
    spawner.spawn(mpsl_task(mpsl).unwrap());
    // USB kræver den eksterne krystal hele tiden.
    if let Ok(hf) = mpsl.request_hfclk().await {
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
    if let Ok(controller) = built {
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
        spawner.spawn(ble::ble_task(controller, mac, name.as_str()).unwrap());
    }

    // ----- USB -----
    spawner.spawn(usb::usb_task(p.USBD, Irqs).unwrap());

    // ----- noden selv -----
    let mut node =
        maarse_fw_common::node::FwNode::new(identity, rng, flash, radio_cfg, radio_ok);
    node.run().await;
}

/// Taler direkte med SX1262 over SPI, før lora-phy overtager: læser sync
/// word, status og fejlflag efter TCXO-start og kalibrering. Resultatet
/// kan ses i status (nøgle 55-58).
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
    // ReadRegister 0x0740 (2 bytes): opcode, adresse, NOP, data, data.
    let mut b = [0x1D, 0x07, 0x40, 0, 0, 0];
    let _ = spi.transfer_in_place(&mut b).await;
    RADIO_DIAG[0].store(u32::from(b[4]) << 8 | u32::from(b[5]), Relaxed);
    wait_busy().await;
    // GetStatus.
    let mut b = [0xC0, 0];
    let _ = spi.transfer_in_place(&mut b).await;
    RADIO_DIAG[1].store(b[1] as u32, Relaxed);
    wait_busy().await;
    // SetDIO3AsTCXOCtrl 1,8 V, timeout 5 ms (320 × 15,625 µs).
    let _ = spi.write(&[0x97, 0x02, 0x00, 0x01, 0x40]).await;
    wait_busy().await;
    // ClearDeviceErrors, Calibrate(alle), vent.
    let _ = spi.write(&[0x07, 0x00, 0x00]).await;
    wait_busy().await;
    let _ = spi.write(&[0x89, 0x7F]).await;
    embassy_time::Timer::after_millis(10).await;
    wait_busy().await;
    // GetDeviceErrors: status, fejl (2 bytes).
    let mut b = [0x17, 0, 0, 0];
    let _ = spi.transfer_in_place(&mut b).await;
    RADIO_DIAG[2].store(u32::from(b[2]) << 8 | u32::from(b[3]), Relaxed);
}
