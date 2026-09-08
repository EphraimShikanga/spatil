use smithay::delegate_data_device;
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};

use crate::state::SpatilState;

impl SelectionHandler for SpatilState {
    type SelectionUserData = ();
}

impl ClientDndGrabHandler for SpatilState {}

impl ServerDndGrabHandler for SpatilState {}

impl DataDeviceHandler for SpatilState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

delegate_data_device!(SpatilState);
