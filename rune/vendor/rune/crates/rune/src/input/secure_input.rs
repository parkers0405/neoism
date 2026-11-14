//! One balanced native secure-input claim per focused, visible password window.
#[derive(Default)]
pub(crate) struct SecureInput(bool);

impl SecureInput {
    pub(crate) fn set(&mut self, active: bool) {
        if active == self.0 { return; }
        #[cfg(target_os = "macos")]
        unsafe {
            if active {
                self.0 = EnableSecureEventInput() == 0;
            } else {
                DisableSecureEventInput();
                self.0 = false;
            }
        }
        #[cfg(not(target_os = "macos"))]
        { self.0 = active; }
    }
}

impl Drop for SecureInput {
    fn drop(&mut self) { self.set(false); }
}

#[cfg(target_os = "macos")]
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn EnableSecureEventInput() -> i32;
    fn DisableSecureEventInput() -> i32;
}
