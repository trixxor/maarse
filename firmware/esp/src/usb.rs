//! USB-Serial-JTAG (the ESP32-S3's built-in USB): companion frames in and out.
//! Shows up as an ordinary serial port on the PC (no driver needed).

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
                    maarse_fw_common::bus::USB_BYTES_IN.fetch_add(n as u32, core::sync::atomic::Ordering::Relaxed);
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
            // If no host is connected, the FIFO fills up; write_all
            // then waits, and the frame is discarded after a second.
            let sent = embassy_time::with_timeout(
                embassy_time::Duration::from_secs(1),
                async {
                    let ok = tx.write_all(&frame).await.is_ok();
                    let _ = tx.flush().await;
                    ok
                },
            )
            .await;
            if sent == Ok(true) {
                maarse_fw_common::bus::USB_BYTES_OUT.fetch_add(frame.len() as u32, core::sync::atomic::Ordering::Relaxed);
            }
        }
    };
    join(reader, writer).await;
}
