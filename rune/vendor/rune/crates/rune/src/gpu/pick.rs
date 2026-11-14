//! Choosing the GPU adapter. On a machine with two GPUs, the one a Wayland compositor renders
//! and scans out with is the one whose buffers it can show: Hyprland on an NVIDIA card shows an
//! integrated GPU's buffers as black rectangles. So Rune asks the compositor for its main device
//! (`zwp_linux_dmabuf_v1` feedback) and takes the adapter on the same PCI device. A laptop that
//! drives its panel from the integrated GPU and offloads to the discrete one gets the integrated
//! GPU, as before.
//!
//! On Windows, a window's or overlay's surface gets the adapter that drives the primary display
//! (the one with the taskbar), else the next display's; offscreen work keeps the lowest-power
//! one. On a hybrid laptop whose monitors hang off the discrete GPU, that's the discrete GPU,
//! for the main window and the HUD alike: an AMD integrated GPU presenting the HUD across to
//! them grew the process by gigabytes over an hour of dictation (hex#1268). A swapchain on a GPU
//! that drives none (a desktop's unused integrated GPU beside the discrete card the monitors
//! hang off) is presented across adapters: each one costs the display GPU's driver a device of
//! its own (about 37 threads with the NVIDIA driver, one of them waking 64 times a second) and
//! about 100 MB per full-screen surface on each GPU, and takes 50 to 160 ms to make instead of
//! 6 ms. A laptop whose panel is on its integrated GPU still gets the integrated GPU.
//!
//! `WGPU_ADAPTER_NAME` (a case-insensitive part of the adapter's name) and `WGPU_POWER_PREF`
//! (`low`, `high` or `none`) override the choice. Without them, and without a compositor device
//! or a display's GPU to match, Rune takes the lowest-power adapter. `RUNE_GPU_LOG=1` prints the choice and its
//! reason; a machine with more than one GPU prints it anyway, once per process.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    EnvName,
    EnvPower,
    Compositor,
    Display,
    LowPower,
}

impl Reason {
    fn describe(self) -> &'static str {
        match self {
            Reason::EnvName => "WGPU_ADAPTER_NAME",
            Reason::EnvPower => "WGPU_POWER_PREF",
            Reason::Compositor => "the compositor's GPU",
            Reason::Display => "the GPU driving the primary display",
            Reason::LowPower => "lowest power",
        }
    }
}

pub(crate) struct Pick {
    pub adapter: wgpu::Adapter,
    pub reason: Reason,
    /// The compositor's main device as a PCI address, when it told us one.
    compositor: Option<String>,
}

impl Pick {
    /// The user chose the adapter; nothing else should swap it.
    pub fn from_env(&self) -> bool {
        matches!(self.reason, Reason::EnvName | Reason::EnvPower)
    }

    /// Whether `info` is an adapter on the compositor's device.
    pub fn on_compositor_device(&self, info: &wgpu::AdapterInfo) -> bool {
        self.compositor.as_deref().is_some_and(|pci| same_pci(pci, &info.device_pci_bus_id))
    }

    /// Whether the compositor can show `info`'s buffers: any adapter's when it named no device,
    /// otherwise only its own device's.
    pub fn shows(&self, info: &wgpu::AdapterInfo) -> bool {
        shows(self.compositor.as_deref(), &info.device_pci_bus_id)
    }
}

/// `Pick::shows` for an adapter on PCI device `pci`.
fn shows(compositor: Option<&str>, pci: &str) -> bool {
    compositor.is_none_or(|compositor| same_pci(compositor, pci))
}

/// The adapter for `surface` (or for offscreen work without one, unless it's for a surface not
/// made yet: `presented`), by the order in the module docs.
pub(crate) fn pick(instance: &wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, presented: bool) -> Result<Pick, String> {
    let usable = |adapter: &wgpu::Adapter| surface.is_none_or(|s| adapter.is_surface_supported(s));
    let compositor = compositor_pci();
    let mut enumerated = None;
    if let Ok(name) = std::env::var("WGPU_ADAPTER_NAME") {
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        let infos: Vec<_> = adapters.iter().map(|a| (a.get_info(), usable(a))).collect();
        let found = by_name(&name, &infos);
        enumerated = Some(infos.len());
        match found {
            Some(index) => return Ok(log(Pick { adapter: adapters.into_iter().nth(index).expect("index from this list"), reason: Reason::EnvName, compositor }, enumerated)),
            None => eprintln!("rune: no GPU adapter matches WGPU_ADAPTER_NAME={name:?}; choosing as usual"),
        }
    }
    if let Some(power) = wgpu::PowerPreference::from_env() {
        let adapter = request(instance, surface, power)?;
        return Ok(log(Pick { adapter, reason: Reason::EnvPower, compositor }, enumerated));
    }
    if let Some(pci) = compositor.as_deref() {
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        let infos: Vec<_> = adapters.iter().map(|a| (a.get_info(), usable(a))).collect();
        enumerated = Some(infos.iter().filter(|(info, _)| info.device_type != wgpu::DeviceType::Cpu).count());
        if let Some(index) = by_pci(pci, &infos) {
            let adapter = adapters.into_iter().nth(index).expect("index from this list");
            return Ok(log(Pick { adapter, reason: Reason::Compositor, compositor }, enumerated));
        }
    }
    // Only a surface is presented; offscreen work (tests, snapshots) keeps the lowest-power GPU.
    let displays = if surface.is_some() || presented { display_devices() } else { Vec::new() };
    if !displays.is_empty() {
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        let infos: Vec<_> = adapters.iter().map(|a| (a.get_info(), usable(a))).collect();
        enumerated = Some(infos.iter().filter(|(info, _)| info.device_type != wgpu::DeviceType::Cpu).count());
        if let Some(index) = by_display(&displays, &infos) {
            let adapter = adapters.into_iter().nth(index).expect("index from this list");
            return Ok(log(Pick { adapter, reason: Reason::Display, compositor }, enumerated));
        }
    }
    let adapter = request(instance, surface, wgpu::PowerPreference::LowPower)?;
    Ok(log(Pick { adapter, reason: Reason::LowPower, compositor }, enumerated))
}

fn request(instance: &wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, power_preference: wgpu::PowerPreference) -> Result<wgpu::Adapter, String> {
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference,
        force_fallback_adapter: false,
        compatible_surface: surface,
        apply_limit_buckets: false,
    }))
    .map_err(|error| format!("no GPU adapter: {error}"))
}

/// The first usable adapter whose name contains `name`, ignoring case.
fn by_name(name: &str, adapters: &[(wgpu::AdapterInfo, bool)]) -> Option<usize> {
    let name = name.to_lowercase();
    adapters.iter().position(|(info, usable)| *usable && info.name.to_lowercase().contains(&name))
}

/// The usable adapter on PCI device `pci`, Vulkan first.
fn by_pci(pci: &str, adapters: &[(wgpu::AdapterInfo, bool)]) -> Option<usize> {
    let on = |(info, usable): &(wgpu::AdapterInfo, bool)| *usable && same_pci(pci, &info.device_pci_bus_id);
    adapters.iter().position(|a| on(a) && a.0.backend == wgpu::Backend::Vulkan).or_else(|| adapters.iter().position(on))
}

/// The usable hardware adapter for the first of `displays` (PCI vendor and device IDs, the
/// primary display's first) that has one.
fn by_display(displays: &[(u32, u32)], adapters: &[(wgpu::AdapterInfo, bool)]) -> Option<usize> {
    displays.iter().find_map(|ids| {
        adapters.iter().position(|(info, usable)| *usable && info.device_type != wgpu::DeviceType::Cpu && (info.vendor, info.device) == *ids)
    })
}

/// `PowerPreference::LowPower`'s order; a software adapter comes last.
pub(crate) fn power_rank(device: wgpu::DeviceType) -> u8 {
    match device {
        wgpu::DeviceType::IntegratedGpu => 0,
        wgpu::DeviceType::DiscreteGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 3,
        wgpu::DeviceType::Cpu => 4,
    }
}

/// The PCI vendor and device IDs of the GPUs driving a display on the desktop, the primary
/// display's first.
#[cfg(target_os = "windows")]
fn display_devices() -> Vec<(u32, u32)> {
    use windows_sys::Win32::Graphics::Gdi::{DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICEW, EnumDisplayDevicesW};
    let mut found = Vec::new();
    for index in 0.. {
        let mut device = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..unsafe { std::mem::zeroed() } };
        // SAFETY: `device` is a DISPLAY_DEVICEW with `cb` set, as the call requires.
        if unsafe { EnumDisplayDevicesW(std::ptr::null(), index, &mut device, 0) } == 0 { break; }
        if device.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 { continue; }
        let len = device.DeviceID.iter().position(|&c| c == 0).unwrap_or(device.DeviceID.len());
        let Some(ids) = pci_ids(&String::from_utf16_lossy(&device.DeviceID[..len])) else { continue };
        if device.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0 {
            found.retain(|other| *other != ids);
            found.insert(0, ids);
        } else if !found.contains(&ids) {
            found.push(ids);
        }
    }
    found
}

#[cfg(not(target_os = "windows"))]
fn display_devices() -> Vec<(u32, u32)> {
    Vec::new()
}

/// The vendor and device IDs in a PnP hardware ID (`PCI\VEN_10DE&DEV_2C05&SUBSYS_…`); None for
/// a display that isn't on a PCI GPU (a remote or indirect display).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn pci_ids(id: &str) -> Option<(u32, u32)> {
    let rest = id.get(..4).filter(|p| p.eq_ignore_ascii_case("PCI\\")).and_then(|_| id.get(4..))?;
    let field = |key: &str| rest.split('&').find_map(|part| {
        let value = part.get(..key.len()).filter(|k| k.eq_ignore_ascii_case(key)).and_then(|_| part.get(key.len()..))?;
        u32::from_str_radix(value, 16).ok()
    });
    Some((field("VEN_")?, field("DEV_")?))
}

fn same_pci(a: &str, b: &str) -> bool {
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

fn log(pick: Pick, adapters: Option<usize>) -> Pick {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if std::env::var_os("RUNE_GPU_LOG").is_some_and(|v| v != "0") || adapters.is_some_and(|n| n > 1) {
        ONCE.call_once(|| {
            let info = pick.adapter.get_info();
            let pci = if info.device_pci_bus_id.is_empty() { String::new() } else { format!(", {}", info.device_pci_bus_id) };
            let compositor = pick.compositor.as_deref().map(|c| format!("; compositor on {c}")).unwrap_or_default();
            eprintln!("rune: GPU {:?} ({}; {:?}, {:?}{pci}): {}{compositor}", info.name, crate::gpu::backend::label(&pick.adapter), info.backend, info.device_type, pick.reason.describe());
        });
    }
    pick
}

/// The compositor's main device as a PCI address, asked once per process.
#[cfg(target_os = "linux")]
fn compositor_pci() -> Option<String> {
    static PCI: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    PCI.get_or_init(|| {
        let (major, minor) = dev_numbers(&dmabuf::main_device()?)?;
        pci_address(&std::fs::read_link(format!("/sys/dev/char/{major}:{minor}/device")).ok()?)
    })
    .clone()
}

#[cfg(not(target_os = "linux"))]
fn compositor_pci() -> Option<String> {
    None
}

/// A `dev_t` sent as native-endian bytes, split into major and minor numbers (glibc's and the
/// kernel's encoding).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn dev_numbers(bytes: &[u8]) -> Option<(u32, u32)> {
    let dev = match bytes.len() {
        8 => u64::from_ne_bytes(bytes.try_into().ok()?),
        4 => u64::from(u32::from_ne_bytes(bytes.try_into().ok()?)),
        _ => return None,
    };
    let major = ((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0x0fff);
    let minor = ((dev >> 12) & 0xffff_ff00) | (dev & 0xff);
    Some((major as u32, minor as u32))
}

/// The PCI address a DRM node's `device` link points at (`../../../0000:01:00.0`), lowercased;
/// None for a device that isn't on PCI.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn pci_address(link: &std::path::Path) -> Option<String> {
    let name = link.file_name()?.to_str()?.to_ascii_lowercase();
    let (domain, rest) = name.split_once(':')?;
    let (bus, rest) = rest.split_once(':')?;
    let (device, function) = rest.split_once('.')?;
    let hex = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit());
    (hex(domain, 4) && hex(bus, 2) && hex(device, 2) && hex(function, 1)).then_some(name)
}

#[cfg(target_os = "linux")]
mod dmabuf {
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_registry::WlRegistry;
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
    use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_feedback_v1::{self, ZwpLinuxDmabufFeedbackV1};
    use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1;

    #[derive(Default)]
    struct State {
        main: Option<Vec<u8>>,
        done: bool,
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }

    impl Dispatch<ZwpLinuxDmabufV1, ()> for State {
        fn event(_: &mut Self, _: &ZwpLinuxDmabufV1, _: <ZwpLinuxDmabufV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }

    impl Dispatch<ZwpLinuxDmabufFeedbackV1, ()> for State {
        fn event(state: &mut Self, _: &ZwpLinuxDmabufFeedbackV1, event: zwp_linux_dmabuf_feedback_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            match event {
                zwp_linux_dmabuf_feedback_v1::Event::MainDevice { device } => state.main = Some(device),
                zwp_linux_dmabuf_feedback_v1::Event::Done => state.done = true,
                _ => {}
            }
        }
    }

    /// The compositor's main device `dev_t`, from its default dmabuf feedback on a short-lived
    /// connection of our own. None off Wayland or before `zwp_linux_dmabuf_v1` version 4.
    /// Version 6 stops sending the main device, so Rune binds at most 5.
    pub(super) fn main_device() -> Option<Vec<u8>> {
        std::env::var_os("WAYLAND_DISPLAY")?;
        let conn = Connection::connect_to_env().ok()?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
        let dmabuf = globals.bind::<ZwpLinuxDmabufV1, _, _>(&queue.handle(), 4..=5, ()).ok()?;
        let feedback = dmabuf.get_default_feedback(&queue.handle(), ());
        let mut state = State::default();
        for _ in 0..4 {
            if state.done { break; }
            queue.roundtrip(&mut state).ok()?;
        }
        feedback.destroy();
        dmabuf.destroy();
        let _ = queue.flush();
        state.main
    }
}

#[cfg(test)]
#[path = "pick_tests.rs"]
mod tests;
