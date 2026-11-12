use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use super::raw::hyprland_lock_notify::v1::server::hyprland_lock_notification_v1::HyprlandLockNotificationV1;
use super::raw::hyprland_lock_notify::v1::server::hyprland_lock_notifier_v1::{
    self, HyprlandLockNotifierV1,
};

const VERSION: u32 = 1;

pub struct HyprlandLockNotifyState {
    notifications: Vec<HyprlandLockNotificationV1>,
}

pub struct HyprlandLockNotifyGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

pub trait HyprlandLockNotifyHandler {
    fn lock_notify_state(&mut self) -> &mut HyprlandLockNotifyState;
    fn lock_notify_is_locked(&self) -> bool;
}

impl HyprlandLockNotifyState {
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<HyprlandLockNotifierV1, HyprlandLockNotifyGlobalData>,
        D: Dispatch<HyprlandLockNotifierV1, ()>,
        D: Dispatch<HyprlandLockNotificationV1, ()>,
        D: HyprlandLockNotifyHandler + 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        display.create_global::<D, HyprlandLockNotifierV1, _>(
            VERSION,
            HyprlandLockNotifyGlobalData {
                filter: Box::new(filter),
            },
        );
        Self {
            notifications: Vec::new(),
        }
    }

    pub fn notify(&mut self, locked: bool) {
        self.notifications
            .retain(|notification| notification.is_alive());
        for notification in &self.notifications {
            if locked {
                notification.locked();
            } else {
                notification.unlocked();
            }
        }
    }
}

impl<D> GlobalDispatch<HyprlandLockNotifierV1, HyprlandLockNotifyGlobalData, D>
    for HyprlandLockNotifyState
where
    D: GlobalDispatch<HyprlandLockNotifierV1, HyprlandLockNotifyGlobalData>,
    D: Dispatch<HyprlandLockNotifierV1, ()>,
    D: Dispatch<HyprlandLockNotificationV1, ()>,
    D: HyprlandLockNotifyHandler + 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        notifier: New<HyprlandLockNotifierV1>,
        _global_data: &HyprlandLockNotifyGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(notifier, ());
    }

    fn can_view(client: Client, global_data: &HyprlandLockNotifyGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<HyprlandLockNotifierV1, (), D> for HyprlandLockNotifyState
where
    D: Dispatch<HyprlandLockNotifierV1, ()>,
    D: Dispatch<HyprlandLockNotificationV1, ()>,
    D: HyprlandLockNotifyHandler + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &HyprlandLockNotifierV1,
        request: hyprland_lock_notifier_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            hyprland_lock_notifier_v1::Request::Destroy => (),
            hyprland_lock_notifier_v1::Request::GetLockNotification { id } => {
                let notification = data_init.init(id, ());
                if state.lock_notify_is_locked() {
                    notification.locked();
                }
                state.lock_notify_state().notifications.push(notification);
            }
        }
    }
}

impl<D> Dispatch<HyprlandLockNotificationV1, (), D> for HyprlandLockNotifyState
where
    D: Dispatch<HyprlandLockNotificationV1, ()>,
    D: HyprlandLockNotifyHandler + 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &HyprlandLockNotificationV1,
        _request: super::raw::hyprland_lock_notify::v1::server::hyprland_lock_notification_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }

    fn destroyed(
        state: &mut D,
        _client: ClientId,
        resource: &HyprlandLockNotificationV1,
        _data: &(),
    ) {
        state
            .lock_notify_state()
            .notifications
            .retain(|item| item != resource);
    }
}

#[macro_export]
macro_rules! delegate_hyprland_lock_notify {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::protocols::raw::hyprland_lock_notify::v1::server::hyprland_lock_notifier_v1::HyprlandLockNotifierV1: $crate::protocols::hyprland_lock_notify::HyprlandLockNotifyGlobalData
        ] => $crate::protocols::hyprland_lock_notify::HyprlandLockNotifyState);

        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::protocols::raw::hyprland_lock_notify::v1::server::hyprland_lock_notifier_v1::HyprlandLockNotifierV1: ()
        ] => $crate::protocols::hyprland_lock_notify::HyprlandLockNotifyState);

        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::protocols::raw::hyprland_lock_notify::v1::server::hyprland_lock_notification_v1::HyprlandLockNotificationV1: ()
        ] => $crate::protocols::hyprland_lock_notify::HyprlandLockNotifyState);
    };
}
