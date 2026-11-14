//! USB-Serial-JTAG (ESP32-S3's indbyggede USB): companion-rammer ind og ud.
//! Vises som en almindelig seriel port på PC'en (ingen driver nødvendig).

use embassy_futures::join::join;
use embedded_io_async::{Read, Write};
use esp_hal::peripherals::USB_DEVICE;
use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use maarse_core::companion::FrameDecoder;
use maarse_fw_common::bus::{USB_IN, USB_OUT};

#[embassy_executor::task]
pub async fn usb_task(usb: USB_DEVICE<'static>) {
    let (mut rx, mut tx) = UsbSerialJtag::new(usb).into_async().split();
    let reader = async {
        let mut dec = FrameDecoder::new();
        let mut buf = [0u8; 64];
        loop {
            match rx.read(&mut buf).await {
                Ok(n) if n > 0 => {
                    dec.push(&buf[..n]);
                    while let Some(f) = dec.next_frame() {
                        maarse_fw_common::bus::USB_LAST_RX.store(
                            embassy_time::Instant::now().as_secs().max(1) as u32,
                            core::sync::atomic::Ordering::Relaxed,
                        );
                        USB_IN.send(f).await;
                    }
                }
                _ => embassy_time::Timer::after_millis(10).await,
            }
        }
    };
    let writer = async {
        loop {
            let frame = USB_OUT.receive().await;
            // Er der ingen vært tilsluttet, fyldes FIFO'en op; write_all
            // venter så, og rammen kasseres efter et sekund.
            let _ = embassy_time::with_timeout(
                embassy_time::Duration::from_secs(1),
                async {
                    let _ = tx.write_all(&frame).await;
                    let _ = tx.flush().await;
                },
            )
            .await;
        }
    };
    join(reader, writer).await;
}
