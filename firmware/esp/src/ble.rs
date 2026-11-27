//! Bluetooth: Nordic UART Service (NUS), so the web client and apps can speak
//! the companion protocol with the node via Bluetooth. Uses esp-radio's
//! BLE controller (HCI) and TrouBLE as host. GATT service, advertising and
//! frames are the same as in the nRF firmware (firmware/nrf/src/ble.rs), so
//! the web client works unchanged.

use embassy_futures::join::join;
use embassy_futures::select::{Either, select};
use bt_hci::controller::ExternalController;
use esp_radio::ble::controller::BleConnector;
use maarse_core::companion::FrameDecoder;
use maarse_fw_common::bus::{BLE_OUT, BLE_PIN_CHANGED, USB_IN};
use trouble_host::prelude::*;

/// Pairing with the node's Bluetooth code (shared with the other firmware).
#[path = "../../common/ble_security.rs"]
mod security;
use security::Security;

/// The largest notification the `tx` characteristic holds.
const CHUNK_MAX: usize = 244;

#[gatt_server]
struct Server {
    nus: NordicUart,
}

#[gatt_service(uuid = "6e400001-b5a3-f393-e0a9-e50e24dcca9e")]
struct NordicUart {
    /// The client writes here.
    #[characteristic(uuid = "6e400002-b5a3-f393-e0a9-e50e24dcca9e", write, write_without_response)]
    rx: heapless09::Vec<u8, 244>,
    /// The node notifies from here.
    #[characteristic(uuid = "6e400003-b5a3-f393-e0a9-e50e24dcca9e", notify)]
    tx: heapless09::Vec<u8, 244>,
}

/// The HCI controller on top of esp-radio.
pub type Ble = ExternalController<BleConnector<'static>, 20>;

#[embassy_executor::task]
pub async fn ble_task(sdc: Ble, mac: [u8; 6], name: &'static str, bonds: maarse_core::blepair::Bonds) {
    maarse_fw_common::bus::BLE_ACTIVE.store(true, core::sync::atomic::Ordering::Relaxed);
    let mut resources: HostResources<DefaultPacketPool, 1, 2> = HostResources::new();
    let stack = trouble_host::new(sdc, &mut resources)
        .set_random_address(Address::random(mac))
        .set_io_capabilities(IoCapabilities::DisplayOnly)
        .build();
    security::restore(&stack, &bonds);
    let mut runner = stack.runner();
    let mut peripheral = stack.peripheral();
    let Ok(server) = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name,
        appearance: &appearance::UNKNOWN,
    })) else {
        return;
    };

    let _ = join(
        async {
            loop {
                let _ = runner.run().await;
            }
        },
        async {
            let mut pairing = Security::default();
            loop {
                pairing.apply(&stack);
                let conn = match select(advertise(name, &mut peripheral, &server), BLE_PIN_CHANGED.wait()).await {
                    Either::First(Ok(conn)) => conn,
                    Either::First(Err(_)) => {
                        embassy_time::Timer::after_secs(1).await;
                        continue;
                    }
                    Either::Second(()) => {
                        pairing.code_changed_idle(&stack);
                        continue;
                    }
                };
                // Old frames from before the connection are thrown away.
                while BLE_OUT.try_receive().is_ok() {}
                maarse_fw_common::bus::BLE_CONNECTED.store(true, core::sync::atomic::Ordering::Relaxed);
                pairing.connected(&stack, conn.raw());
                select(rx_task(&server, &conn, &stack, &mut pairing), tx_task(&server, &conn)).await;
                // The client is gone: no longer anyone's modem.
                maarse_fw_common::bus::BLE_LOST.store(true, core::sync::atomic::Ordering::Relaxed);
                maarse_fw_common::bus::BLE_CONNECTED.store(false, core::sync::atomic::Ordering::Relaxed);
            }
        },
    )
    .await;
}

async fn advertise<'v, 's, C: Controller>(
    name: &'v str,
    peripheral: &mut Peripheral<'v, C, DefaultPacketPool>,
    server: &'s Server<'v>,
) -> Result<GattConnection<'v, 's, DefaultPacketPool>, BleHostError<C::Error>> {
    let mut adv = [0u8; 31];
    let len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(name.as_bytes()),
        ],
        &mut adv[..],
    )?;
    // The NUS UUID is sent in the scan response, so the name can be in the advertisement.
    let mut scan = [0u8; 31];
    let nus: [u8; 16] = [
        0x9e, 0xca, 0xdc, 0x24, 0x0e, 0xe5, 0xa9, 0xe0, 0x93, 0xf3, 0xa3, 0xb5, 0x01, 0x00, 0x40, 0x6e,
    ];
    let slen = AdStructure::encode_slice(&[AdStructure::IncompleteServiceUuids128(&[nus])], &mut scan[..])?;
    let advertiser = peripheral
        .advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected { adv_data: &adv[..len], scan_data: &scan[..slen] },
        )
        .await?;
    let conn = advertiser.accept().await?.with_attribute_server(server)?;
    Ok(conn)
}

/// Data from the client: collect frames and send them to the node.
async fn rx_task<C: Controller>(
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    stack: &Stack<'_, C, DefaultPacketPool>,
    pairing: &mut Security,
) {
    let rx_handle = server.nus.rx.handle;
    let mut dec = FrameDecoder::new();
    loop {
        let event = match select(conn.next(), BLE_PIN_CHANGED.wait()).await {
            Either::First(event) => event,
            Either::Second(()) => {
                if pairing.code_changed(stack, conn.raw()) {
                    continue;
                }
                // Changed from elsewhere: let the replies go out, then the
                // client starts over with the new code.
                embassy_time::Timer::after_millis(600).await;
                conn.raw().disconnect();
                return;
            }
        };
        pairing.event(stack, &event);
        match event {
            GattConnectionEvent::Disconnected { .. } => return,
            GattConnectionEvent::Gatt { event } => {
                // Nothing is written before pairing with the code: the error
                // makes the phone pair and try again.
                if matches!(event, GattEvent::Write(_)) && !security::allowed(conn.raw()) {
                    if let Ok(reply) = event.reject(AttErrorCode::INSUFFICIENT_AUTHENTICATION) {
                        reply.send().await;
                    }
                    continue;
                }
                if let GattEvent::Write(w) = &event
                    && w.handle() == rx_handle
                {
                    w.with_data(|_, data| {
                        maarse_fw_common::bus::BLE_BYTES_IN.fetch_add(data.len() as u32, core::sync::atomic::Ordering::Relaxed);
                        dec.push(data)
                    });
                    while let Some(f) = dec.next_frame() {
                        if pairing.accept(&f) {
                            USB_IN.send(f).await;
                        }
                    }
                }
                if let Ok(reply) = event.accept() {
                    reply.send().await;
                }
            }
            _ => {}
        }
    }
}

/// Events from the node: send them as notifications, as large as the link carries.
async fn tx_task(server: &Server<'_>, conn: &GattConnection<'_, '_, DefaultPacketPool>) {
    let tx = &server.nus.tx;
    loop {
        let frame = BLE_OUT.receive().await;
        // Nothing reaches a client that has not paired, and with the factory
        // code no messages or settings.
        if !security::allowed(conn.raw()) || !security::may_send(&frame) {
            continue;
        }
        maarse_fw_common::bus::BLE_BYTES_OUT.fetch_add(frame.len() as u32, core::sync::atomic::Ordering::Relaxed);
        // A notification carries the ATT MTU less 3 bytes: 20 until the phone has asked for
        // more (the default MTU is 23). Read for each frame, as the phone asks after connecting.
        let size = usize::from(conn.raw().att_mtu()).saturating_sub(3).clamp(20, CHUNK_MAX);
        for chunk in frame.chunks(size) {
            let mut v: heapless09::Vec<u8, CHUNK_MAX> = heapless09::Vec::new();
            let _ = v.extend_from_slice(chunk);
            match select(tx.notify(conn, &v, false), embassy_time::Timer::after_secs(2)).await {
                Either::First(Ok(())) => {}
                _ => return,
            }
        }
    }
}
