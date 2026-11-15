//! The LoRa task for the nRF52840: concrete types around the shared radio loop.

use embassy_nrf::gpio::Output;
use embassy_nrf::spim::Spim;
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use lora_phy::LoRa;
use lora_phy::iv::GenericSx126xInterfaceVariant;
use lora_phy::sx126x::{Sx126x, Sx1262};
use maarse_core::companion::RadioConfig;

pub type Radio = LoRa<
    Sx126x<
        ExclusiveDevice<Spim<'static>, Output<'static>, Delay>,
        GenericSx126xInterfaceVariant<Output<'static>, embassy_nrf::gpio::Input<'static>>,
        Sx1262,
    >,
    Delay,
>;

#[embassy_executor::task]
pub async fn radio_task(
    mut lora: Radio,
    mut led: Output<'static>,
    led_on_high: bool,
    seed: u64,
    cfg: RadioConfig,
) {
    let set = move |on: bool| {
        if on == led_on_high {
            led.set_high()
        } else {
            led.set_low()
        }
    };
    maarse_fw_common::radio::radio_loop(&mut lora, set, seed, cfg).await;
}
