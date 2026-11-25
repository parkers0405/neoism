use std::cell::RefCell;

use objc2::rc::{Retained, Weak};
use objc2::runtime::NSObject;
use objc2::{declare_class, mutability, ClassType, DeclaredClass};
use objc2_foundation::{ns_string, MainThreadMarker, NSBundle, NSObjectProtocol};
use objc2_ui_kit::{
    UIScene, UISceneConnectionOptions, UISceneDelegate, UISceneSession, UIWindow, UIWindowScene,
};

use super::app_state::{self, EventWrapper};
use super::window::WinitUIWindow;
use crate::event::Event;

thread_local! {
    static WINDOW_SCENE: RefCell<Option<Retained<UIWindowScene>>> = const { RefCell::new(None) };
    static DISCONNECTED_WINDOWS: RefCell<Vec<Weak<UIWindow>>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn uses_scene_lifecycle() -> bool {
    unsafe {
        NSBundle::mainBundle()
            .objectForInfoDictionaryKey(ns_string!("UIApplicationSceneManifest"))
            .is_some()
    }
}

pub(super) fn window_scene(_mtm: MainThreadMarker) -> Option<Retained<UIWindowScene>> {
    WINDOW_SCENE.with(|scene| scene.borrow().clone())
}

declare_class!(
    pub(super) struct WinitSceneDelegate;

    unsafe impl ClassType for WinitSceneDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "WinitSceneDelegate";
    }

    impl DeclaredClass for WinitSceneDelegate {}
    unsafe impl NSObjectProtocol for WinitSceneDelegate {}

    #[allow(non_snake_case)] // Method names match the bindings for UISceneDelegate.
    unsafe impl UISceneDelegate for WinitSceneDelegate {
        #[method(scene:willConnectToSession:options:)]
        fn scene_willConnectToSession_options(
            &self,
            scene: &UIScene,
            _session: &UISceneSession,
            _options: &UISceneConnectionOptions,
        ) {
            if !scene.is_kind_of::<UIWindowScene>() {
                return;
            }
            // SAFETY: The class check above confirms UIKit supplied a UIWindowScene.
            let scene = unsafe { Retained::cast::<UIWindowScene>(scene.retain()) };
            WINDOW_SCENE.with(|current| current.replace(Some(scene.clone())));

            let windows = DISCONNECTED_WINDOWS.with(|windows| windows.take());
            for window in windows.into_iter().filter_map(|window| window.load()) {
                unsafe { window.setWindowScene(Some(&scene)) };
            }

            // UIApplicationDidFinishLaunching precedes the scene connection. Only now can
            // winit hand the application a lifecycle event that lets it create a window.
            app_state::did_finish_launching(MainThreadMarker::from(self));
        }

        #[method(sceneDidDisconnect:)]
        fn sceneDidDisconnect(&self, scene: &UIScene) {
            let Some(current) = window_scene(MainThreadMarker::from(self)) else { return };
            let current_scene: &UIScene = &current;
            if !std::ptr::eq(current_scene, scene) {
                return;
            }
            let windows = unsafe { current.windows() };
            DISCONNECTED_WINDOWS.with(|disconnected| {
                disconnected.replace(
                    windows.iter().filter(|w| w.is_kind_of::<WinitUIWindow>()).map(Weak::new).collect(),
                );
            });
            WINDOW_SCENE.with(|current| current.take());
        }

        #[method(sceneDidBecomeActive:)]
        fn sceneDidBecomeActive(&self, _scene: &UIScene) {
            app_state::handle_nonuser_event(
                MainThreadMarker::from(self), EventWrapper::StaticEvent(Event::Resumed),
            );
        }

        #[method(sceneWillResignActive:)]
        fn sceneWillResignActive(&self, _scene: &UIScene) {
            app_state::handle_nonuser_event(
                MainThreadMarker::from(self), EventWrapper::StaticEvent(Event::Suspended),
            );
        }

        #[method(sceneWillEnterForeground:)]
        fn sceneWillEnterForeground(&self, _scene: &UIScene) {
            self.send_occluded(false);
        }

        #[method(sceneDidEnterBackground:)]
        fn sceneDidEnterBackground(&self, _scene: &UIScene) {
            self.send_occluded(true);
        }
    }
);

impl WinitSceneDelegate {
    fn send_occluded(&self, occluded: bool) {
        let mtm = MainThreadMarker::from(self);
        if let Some(scene) = window_scene(mtm) {
            let windows = unsafe { scene.windows() };
            app_state::send_occluded_events(mtm, &windows, occluded);
        }
    }
}