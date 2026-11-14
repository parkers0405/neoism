use super::{by_display, by_name, by_pci, dev_numbers, pci_address, pci_ids, power_rank, shows};
use std::path::Path;

fn adapter(name: &str, backend: wgpu::Backend, pci: &str) -> wgpu::AdapterInfo {
    wgpu::AdapterInfo { name: name.into(), backend, device_pci_bus_id: pci.into(), ..wgpu::AdapterInfo::new(wgpu::DeviceType::Other, backend) }
}

/// glibc's `makedev`.
fn makedev(major: u64, minor: u64) -> u64 {
    ((major & 0xffff_f000) << 32) | ((major & 0x0fff) << 8) | ((minor & 0xffff_ff00) << 12) | (minor & 0xff)
}

#[test]
fn dev_t_bytes_split_into_major_and_minor() {
    assert_eq!(dev_numbers(&makedev(226, 128).to_ne_bytes()), Some((226, 128)));
    assert_eq!(dev_numbers(&makedev(226, 1).to_ne_bytes()), Some((226, 1)));
    assert_eq!(dev_numbers(&makedev(0x12345, 0x6789a).to_ne_bytes()), Some((0x12345, 0x6789a)));
    assert_eq!(dev_numbers(&((226u32 << 8) | 128).to_ne_bytes()), Some((226, 128)));
    assert_eq!(dev_numbers(&[1, 2, 3]), None);
}

#[test]
fn drm_device_links_give_their_pci_address() {
    assert_eq!(pci_address(Path::new("../../../0000:01:00.0")).as_deref(), Some("0000:01:00.0"));
    assert_eq!(pci_address(Path::new("../../../0000:C5:00.0")).as_deref(), Some("0000:c5:00.0"));
    assert_eq!(pci_address(Path::new("../../../simple-framebuffer.0")), None);
    assert_eq!(pci_address(Path::new("../../../fd4ab000.gpu")), None);
}

#[test]
fn the_compositor_device_picks_the_adapter_on_it() {
    let adapters = [
        (adapter("AMD Radeon Graphics (RADV RAPHAEL_MENDOCINO)", wgpu::Backend::Vulkan, "0000:0f:00.0"), true),
        (adapter("NVIDIA GeForce RTX 5070", wgpu::Backend::Gl, "0000:01:00.0"), true),
        (adapter("NVIDIA GeForce RTX 5070", wgpu::Backend::Vulkan, "0000:01:00.0"), true),
        (adapter("llvmpipe", wgpu::Backend::Vulkan, ""), true),
    ];
    assert_eq!(by_pci("0000:01:00.0", &adapters), Some(2), "Vulkan before GL");
    assert_eq!(by_pci("0000:0F:00.0", &adapters), Some(0));
    assert_eq!(by_pci("0000:02:00.0", &adapters), None);
    assert_eq!(by_pci("", &adapters), None, "no address matches llvmpipe's empty one");
}

#[test]
fn an_adapter_that_cant_present_is_skipped() {
    let adapters = [
        (adapter("NVIDIA GeForce RTX 5070", wgpu::Backend::Vulkan, "0000:01:00.0"), false),
        (adapter("AMD Radeon Graphics", wgpu::Backend::Vulkan, "0000:0f:00.0"), true),
    ];
    assert_eq!(by_pci("0000:01:00.0", &adapters), None);
    assert_eq!(by_name("nvidia", &adapters), None);
    assert_eq!(by_name("RADEON", &adapters), Some(1));
}

/// Once the compositor names its GPU, only that GPU's buffers can be shown: an integrated GPU
/// or lavapipe (no PCI device) never takes an overlay on GNOME + NVIDIA (hex#979, #982).
#[test]
fn only_the_compositors_gpu_shows_once_it_named_one() {
    assert!(shows(Some("0000:01:00.0"), "0000:01:00.0"));
    assert!(!shows(Some("0000:01:00.0"), "0000:7c:00.0"));
    assert!(!shows(Some("0000:01:00.0"), ""));
    assert!(shows(None, "0000:7c:00.0"));
    assert!(shows(None, ""));
}

#[test]
fn low_power_order_puts_integrated_first_and_software_last() {
    use wgpu::DeviceType::*;
    let mut devices = [Cpu, DiscreteGpu, Other, IntegratedGpu, VirtualGpu];
    devices.sort_by_key(|d| power_rank(*d));
    assert_eq!(devices, [IntegratedGpu, DiscreteGpu, VirtualGpu, Other, Cpu]);
}

#[test]
fn a_pci_hardware_id_gives_its_vendor_and_device() {
    assert_eq!(pci_ids("PCI\\VEN_10DE&DEV_2C05&SUBSYS_205E1458&REV_A1"), Some((0x10de, 0x2c05)));
    assert_eq!(pci_ids("pci\\ven_1002&dev_13c0&subsys_d0001458&rev_c9"), Some((0x1002, 0x13c0)));
    assert_eq!(pci_ids("SWD\\RemoteDisplayEnum"), None, "a remote display");
    assert_eq!(pci_ids("PCI\\VEN_10DE"), None);
    assert_eq!(pci_ids(""), None);
}

fn gpu(name: &str, device_type: wgpu::DeviceType, vendor: u32, device: u32) -> wgpu::AdapterInfo {
    wgpu::AdapterInfo { name: name.into(), vendor, device, ..wgpu::AdapterInfo::new(device_type, wgpu::Backend::Dx12) }
}

/// A desktop or laptop whose primary monitor is on its discrete card draws there, not on the
/// integrated GPU (hex#1268); a laptop whose primary display is its panel on the integrated GPU
/// keeps it.
#[test]
fn the_gpu_driving_the_primary_display_is_picked() {
    use wgpu::DeviceType::*;
    let adapters = [
        (gpu("NVIDIA GeForce RTX 5070 Ti", DiscreteGpu, 0x10de, 0x2c05), true),
        (gpu("AMD Radeon(TM) Graphics", IntegratedGpu, 0x1002, 0x13c0), true),
        (gpu("Microsoft Basic Render Driver", Cpu, 0x1414, 0x8c), true),
    ];
    assert_eq!(by_display(&[(0x10de, 0x2c05)], &adapters), Some(0), "only the discrete card drives a display");
    assert_eq!(by_display(&[(0x10de, 0x2c05), (0x1002, 0x13c0)], &adapters), Some(0), "both do: the primary display's");
    assert_eq!(by_display(&[(0x1002, 0x13c0), (0x10de, 0x2c05)], &adapters), Some(1), "the laptop panel is primary");
    assert_eq!(by_display(&[(0x8086, 0x56a0), (0x10de, 0x2c05)], &adapters), Some(0), "the primary display's GPU has no adapter: the next display's");
    assert_eq!(by_display(&[(0x1002, 0x13c0)], &adapters), Some(1), "a laptop panel");
    assert_eq!(by_display(&[(0x1414, 0x8c)], &adapters), None, "never the software adapter");
    assert_eq!(by_display(&[(0x8086, 0x56a0)], &adapters), None, "no adapter drives it");
    let cant_present = [(adapters[0].0.clone(), false), adapters[1].clone()];
    assert_eq!(by_display(&[(0x10de, 0x2c05)], &cant_present), None);
}
