//! USB seriel (CDC ACM): companion-rammer ind og ud.

use embassy_futures::join::join4;
use embassy_nrf::Peri;
use embassy_nrf::peripherals::USBD;
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, Config};
use static_cell::StaticCell;
use maarse_core::companion::FrameDecoder;
use maarse_fw_common::bus::{USB_IN, USB_OUT};

/// Adafruit-bootloaderen starter i UF2-tilstand, når GPREGRET er 0x57.
fn enter_uf2_bootloader() -> ! {
    const GPREGRET: *mut u32 = 0x4000_051C as *mut u32;
    unsafe { core::ptr::write_volatile(GPREGRET, 0x57) };
    cortex_m::peripheral::SCB::sys_reset();
}

#[embassy_executor::task]
pub async fn usb_task(usbd: Peri<'static, USBD>, irqs: crate::Irqs) {
    // CLOCK_POWER-afbrydelsen ejes af MPSL (Bluetooth), så USB-strøm kan
    // ikke opdages i hardware. Vi antager, at kablet er sat i.
    static VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();
    let vbus = VBUS.init(SoftwareVbusDetect::new(true, true));
    let driver = Driver::new(usbd, irqs, &*vbus);
    let mut config = Config::new(0x1209, 0x7a6e); // pid.codes testnummer
    config.manufacturer = Some("Maarse");
    config.product = Some("Maarse-node");
    config.serial_number = Some("maarse");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 64]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static STATE: StaticCell<State> = StaticCell::new();

    let mut builder = Builder::new(
        driver,
        config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        MSOS_DESC.init([0; 64]),
        CONTROL_BUF.init([0; 64]),
    );
    let class = CdcAcmClass::new(&mut builder, STATE.init(State::new()), 64);
    let mut usb = builder.build();
    let (tx, mut rx, ctrl) = class.split_with_control();
    // Deles mellem skriveren og 1200 baud-tjekket (som læser baudraten).
    let tx = embassy_sync::mutex::Mutex::<embassy_sync::blocking_mutex::raw::NoopRawMutex, _>::new(tx);

    let reader = async {
        let mut dec = FrameDecoder::new();
        let mut buf = [0u8; 64];
        loop {
            rx.wait_connection().await;
            while let Ok(n) = rx.read_packet(&mut buf).await {
                dec.push(&buf[..n]);
                while let Some(f) = dec.next_frame() {
                    USB_IN.send(f).await;
                }
            }
        }
    };
    let writer = async {
        loop {
            let frame = USB_OUT.receive().await;
            let mut tx = tx.lock().await;
            let mut ok = true;
            for chunk in frame.chunks(64) {
                if tx.write_packet(chunk).await.is_err() {
                    ok = false;
                    break;
                }
            }
            // En pakke på præcis 64 bytes skal afsluttes med en tom pakke.
            if ok && frame.len() % 64 == 0 {
                let _ = tx.write_packet(&[]).await;
            }
        }
    };
    // "1200 baud-touch": åbner værten porten med 1200 baud og lukker den
    // igen (DTR falder), genstarter vi i UF2-bootloaderen. Så kan
    // scripts/flash-nrf flashe uden at man trykker på reset.
    let touch = async {
        loop {
            ctrl.control_changed().await;
            if !ctrl.dtr() && tx.lock().await.line_coding().data_rate() == 1200 {
                embassy_time::Timer::after_millis(50).await;
                enter_uf2_bootloader();
            }
        }
    };
    join4(usb.run(), reader, writer, touch).await;
}
