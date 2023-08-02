#![no_std]
extern crate alloc;

use core::time::Duration;
use hal_core::{boot::BootInfo, PAddr};
use hal_x86_64::cpu::local::GsLocalData;
pub use hal_x86_64::{
    cpu::{local::LocalKey, wait_for_interrupt},
    mm,
};
use kernel::{
    mnemos_alloc::{
        containers::Box,
        heap::{MnemosAlloc, SingleThreadedLinkedListAllocator},
    },
    Kernel, KernelSettings,
};

pub mod acpi;
pub mod interrupt;

// TODO(eliza): single-threaded linked list allocator is not gonna be sufficient
// on x86 systems with Big memory amounts and SMP...
// TODO(eliza): we will also need a page frame allocator to placate mycelium-hal...
#[global_allocator]
static AHEAP: MnemosAlloc<SingleThreadedLinkedListAllocator> = MnemosAlloc::new();

pub fn init(bootinfo: &impl BootInfo, rsdp_addr: Option<PAddr>) -> &'static Kernel {
    // TODO: init early tracing?
    interrupt::enable_exceptions();
    bootinfo.init_paging();

    // TODO: init allocator!

    // TODO: PCI?

    init_acpi(bootinfo, rsdp_addr);

    // init boot processor's core-local data
    unsafe {
        GsLocalData::init();
    }
    tracing::info!("set up the boot processor's local data");

    let k = {
        let settings = KernelSettings {
            max_drivers: 64, // we are a big x86 system with lots of RAM, this can probably be an even bigger number!
            timer_granularity: Duration::from_millis(10),
        };

        unsafe {
            Box::into_raw(Kernel::new(settings).expect("cannot initialize kernel"))
                .as_ref()
                .unwrap()
        }
    };

    // TODO: spawn drivers (UART, keyboard, ...)
    k
}

pub fn run(bootinfo: &impl BootInfo, k: &'static Kernel) -> ! {
    loop {
        // Tick the scheduler
        // TODO(eliza): do we use the PIT or the local APIC timer?
        let start: Duration = todo!("current value of freewheeling timer");
        let tick = k.tick();

        // Timer is downcounting
        let elapsed = start - todo!("timer current value");
        let turn = k.timer().force_advance(elapsed);

        // If there is nothing else scheduled, and we didn't just wake something up,
        // sleep for some amount of time
        if turn.expired == 0 && !tick.has_remaining {
            let wfi_start: Duration = todo!("timer current value");

            // TODO(AJM): Sometimes there is no "next" in the timer wheel, even though there should
            // be. Don't take lack of timer wheel presence as the ONLY heuristic of whether we
            // should just wait for SOME interrupt to occur. For now, force a max sleep of 100ms
            // which is still probably wrong.
            let amount = turn
                .ticks_to_next_deadline()
                .unwrap_or(todo!("figure this out"));

            todo!("reset timer");

            unsafe {
                interrupt::wait_for_interrupt();
            }
            // Disable the timer interrupt in case that wasn't what woke us up
            todo!("clear timer irq");

            // Account for time slept
            let elapsed = wfi_start - todo!("current timer value");
            let _turn = k.timer().force_advance(elapsed);
        }
    }
}

fn init_acpi(bootinfo: &impl BootInfo, rsdp_addr: Option<PAddr>) {
    if let Some(rsdp) = rsdp_addr {
        let acpi = acpi::acpi_tables(rsdp);
        let platform_info = acpi.and_then(|acpi| acpi.platform_info());
        match platform_info {
            Ok(platform) => {
                tracing::debug!("found ACPI platform info");
                interrupt::enable_hardware_interrupts(Some(&platform.interrupt_model));
                acpi::bringup_smp(&platform)
                    .expect("failed to bring up application processors! this is bad news!");
                return;
            }
            Err(error) => tracing::warn!(?error, "missing ACPI platform info"),
        }
    } else {
        // TODO(eliza): try using MP Table to bringup application processors?
        tracing::warn!("no RSDP from bootloader, skipping SMP bringup");
    }

    // no ACPI
    interrupt::enable_hardware_interrupts(None)
}
