//! Start-up every demo shares.

/// Starts a demo: HAL at full clock, logger, the internal heap in the RAM
/// the bootloader used (`board::RECLAIMED_HEAP`), esp-rtos on TIMG0, a wait
/// for the console to reconnect (`board::wait_for_console`), then logs the
/// title. Binds the peripherals to `$p`. Must be called from an async fn.
///
/// ```ignore
/// esp32_p4_function_ev_examples::start!(p, "Demo 00: bring-up");
/// ```
#[macro_export]
macro_rules! start {
    ($p:ident, $title:expr) => {
        let $p = esp32_p4_function_ev::board::init();
        esp32_p4_function_ev::board::init_logger();
        esp_alloc::heap_allocator!(
            #[esp_hal::ram(reclaimed)]
            size: esp32_p4_function_ev::board::RECLAIMED_HEAP
        );
        let timg0 = esp_hal::timer::timg::TimerGroup::new($p.TIMG0);
        esp_rtos::start(timg0.timer0, $p.FROM_CPU_INTR0);
        esp32_p4_function_ev::board::wait_for_console().await;
        log::info!("=== {} ===", $title);
    };
}
