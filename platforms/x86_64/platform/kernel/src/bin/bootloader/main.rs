#![no_std]
#![no_main]

#[cfg(not(feature = "bootloader_api"))]
compile_error!(
    "building the `mnemos-x86_64-bootloader` binary requires the \
    'bootloader_api' Cargo feature to be enabled",
);
use bootloader_api::config::{BootloaderConfig, Mapping};
use hal_core::PAddr;
use hal_x86_64::cpu;
mod bootinfo;
mod framebuf;

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    // the kernel is mapped into the higher half of the virtual address space.
    config.mappings.dynamic_range_start = Some(0xFFFF_8000_0000_0000);
    config.mappings.page_table_recursive = Some(Mapping::Dynamic);

    config
};

bootloader_api::entry_point!(kernel_start, config = &BOOTLOADER_CONFIG);

pub fn kernel_start(info: &'static mut bootloader_api::BootInfo) -> ! {
    unsafe {
        cpu::intrinsics::cli();
    }

    let rsdp_addr = info.rsdp_addr.into_option().map(PAddr::from_u64);
    let bootinfo = bootinfo::BootloaderApiBootInfo::from_bootloader(info);

    let k = mnemos_x86_64_core::init(&bootinfo, rsdp_addr);
    mnemos_x86_64_core::run(&bootinfo, k)
}

#[cold]
#[cfg_attr(target_os = "none", panic_handler)]
fn panic(panic: &core::panic::PanicInfo<'_>) -> ! {
    use embedded_graphics::{
        mono_font::{ascii, MonoTextStyleBuilder},
        pixelcolor::{Rgb888, RgbColor as _},
        prelude::*,
        text::{Alignment, Text},
    };

    use hal_core::framebuffer::{Draw, RgbColor};

    // /!\ disable all interrupts, unlock everything to prevent deadlock /!\
    //
    // Safety: it is okay to do this because we are panicking and everything
    // is going to die anyway.
    unsafe {
        // disable all interrupts.
        cpu::intrinsics::cli();

        // TODO(eliza): claim serial

        // unlock the frame buffer
        framebuf::force_unlock();
    }

    let mut framebuf = unsafe { framebuf::mk_framebuf() };
    framebuf.fill(RgbColor::RED);

    let mut target = framebuf.as_draw_target();

    let style = MonoTextStyleBuilder::new()
        .font(&ascii::FONT_9X15_BOLD)
        .text_color(Rgb888::RED)
        .background_color(Rgb888::WHITE)
        .build();

    let _ = Text::with_alignment("mnemos panicked", Point::new(5, 15), style, Alignment::Left)
        .draw(&mut target)
        .unwrap();

    // TODO(eliza): actually print something good

    unsafe {
        cpu::halt();
    }
}
