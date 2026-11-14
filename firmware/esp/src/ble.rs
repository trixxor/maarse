//! Bluetooth: Nordic UART Service (NUS), så webklienten og apps kan tale
//! companion-protokollen med noden via Bluetooth. Bruger esp-radios
//! BLE-controller (HCI) og TrouBLE som host. GATT-tjeneste, reklame og
//! rammer er de samme som i nRF-firmwaren (firmware/nrf/src/ble.rs), så
//! webklienten virker uændret.

use embassy_futures::join::join;
use embassy_futures::select::{Either, select};
use bt_hci::controller::ExternalController;
use esp_radio::ble::controller::BleConnector;
use maarse_core::companion::FrameDecoder;
use maarse_fw_common::bus::{BLE_OUT, USB_IN};
use trouble_host::prelude::*;

/// Bidder på 20 bytes passer altid (standard ATT-MTU 23).
const CHUNK: usize = 20;

#[gatt_server]
struct Server {
    nus: NordicUart,
}

#[gatt_service(uuid = "6e400001-b5a3-f393-e0a9-e50e24dcca9e")]
struct NordicUart {
    /// Klienten skriver hertil.
    #[characteristic(uuid = "6e400002-b5a3-f393-e0a9-e50e24dcca9e", write, write_without_response)]
    rx: heapless09::Vec<u8, 244>,
    /// Noden notificerer herfra.
    #[characteristic(uuid = "6e400003-b5a3-f393-e0a9-e50e24dcca9e", notify)]
    tx: heapless09::Vec<u8, 244>,
}

/// HCI-controlleren oven på esp-radio.
pub type Ble = ExternalController<BleConnector<'static>, 20>;

#[embassy_executor::task]
pub async fn ble_task(sdc: Ble, mac: [u8; 6], name: &'static str) {
    let mut resources: HostResources<DefaultPacketPool, 1, 2> = HostResources::new();
    let stack = trouble_host::new(sdc, &mut resources)
        .set_random_address(Address::random(mac))
        .build();
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
            loop {
                let Ok(conn) = advertise(name, &mut peripheral, &server).await else {
                    embassy_time::Timer::after_secs(1).await;
                    continue;
                };
                // Gamle rammer fra før forbindelsen smides væk.
                while BLE_OUT.try_receive().is_ok() {}
                select(rx_task(&server, &conn), tx_task(&server, &conn)).await;
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
    // NUS-UUID'en sendes i scan-svaret, så navnet kan være i reklamen.
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

/// Data fra klienten: saml rammer og send dem til noden.
async fn rx_task<P: PacketPool>(server: &Server<'_>, conn: &GattConnection<'_, '_, P>) {
    let rx_handle = server.nus.rx.handle;
    let mut dec = FrameDecoder::new();
    loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { .. } => return,
            GattConnectionEvent::Gatt { event } => {
                if let GattEvent::Write(w) = &event
                    && w.handle() == rx_handle
                {
                    w.with_data(|_, data| dec.push(data));
                    while let Some(f) = dec.next_frame() {
                        USB_IN.send(f).await;
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

/// Hændelser fra noden: send dem som notifikationer i små bidder.
async fn tx_task<P: PacketPool>(server: &Server<'_>, conn: &GattConnection<'_, '_, P>) {
    let tx = &server.nus.tx;
    loop {
        let frame = BLE_OUT.receive().await;
        for chunk in frame.chunks(CHUNK) {
            let mut v: heapless09::Vec<u8, 244> = heapless09::Vec::new();
            let _ = v.extend_from_slice(chunk);
            match select(tx.notify(conn, &v, false), embassy_time::Timer::after_secs(2)).await {
                Either::First(Ok(())) => {}
                _ => return,
            }
        }
    }
}
