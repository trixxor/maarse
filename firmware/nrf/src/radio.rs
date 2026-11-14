//! LoRa-opgaven til nRF52840: konkrete typer omkring den fælles radio-løkke.

use embassy_nrf::gpio::Output;
use embassy_nrf::spim::Spim;
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use lora_phy::LoRa;
use lora_phy::iv::GenericSx126xInterfaceVariant;
use lora_phy::sx126x::{Sx126x, Sx1262};
use maarse_core::companion::RadioConfig;

/// Input-ben, der venter ved at aflæse benet hvert millisekund i stedet for
/// at bruge GPIOTE-interrupt. Fejlsøgning: kan udelukke, at interruptet fra
/// DIO1 går tabt.
pub struct PollPin(pub embassy_nrf::gpio::Input<'static>);

impl embedded_hal::digital::ErrorType for PollPin {
    type Error = core::convert::Infallible;
}

impl embedded_hal_async::digital::Wait for PollPin {
    async fn wait_for_high(&mut self) -> Result<(), Self::Error> {
        while self.0.is_low() {
            embassy_time::Timer::after_millis(1).await;
        }
        Ok(())
    }
    async fn wait_for_low(&mut self) -> Result<(), Self::Error> {
        while self.0.is_high() {
            embassy_time::Timer::after_millis(1).await;
        }
        Ok(())
    }
    async fn wait_for_rising_edge(&mut self) -> Result<(), Self::Error> {
        self.wait_for_low().await?;
        self.wait_for_high().await
    }
    async fn wait_for_falling_edge(&mut self) -> Result<(), Self::Error> {
        self.wait_for_high().await?;
        self.wait_for_low().await
    }
    async fn wait_for_any_edge(&mut self) -> Result<(), Self::Error> {
        if self.0.is_high() {
            self.wait_for_low().await
        } else {
            self.wait_for_high().await
        }
    }
}

pub type Radio = LoRa<
    Sx126x<
        ExclusiveDevice<Spim<'static>, Output<'static>, Delay>,
        GenericSx126xInterfaceVariant<Output<'static>, PollPin>,
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
