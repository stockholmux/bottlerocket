//! The `v3` module contains the third version of the network configuration and implements the
//! appropriate traits.

use super::devices::NetworkDeviceV1;
use super::{error, Interfaces, Result, Validate};
use crate::interface_name::InterfaceName;
use crate::wicked::{WickedInterface, WickedLinkConfig};
use indexmap::IndexMap;
use serde::Deserialize;
use snafu::ensure;
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
pub(crate) struct NetConfigV3 {
    #[serde(flatten)]
    pub(crate) net_devices: IndexMap<InterfaceName, NetworkDeviceV1>,
}

impl Interfaces for NetConfigV3 {
    fn primary_interface(&self) -> Option<String> {
        self.net_devices
            .iter()
            .find(|(_, v)| v.primary() == Some(true))
            .or_else(|| self.net_devices.first())
            .map(|(n, _)| n.to_string())
    }

    fn has_interfaces(&self) -> bool {
        !self.net_devices.is_empty()
    }

    fn as_wicked_interfaces(&self) -> Vec<WickedInterface> {
        let mut wicked_interfaces = Vec::new();
        for (name, config) in &self.net_devices {
            let interface = WickedInterface::from((name, config));

            // If config is a Bond, we will generate the interface configuration for interfaces in
            // that bond since we have all of the data and the bond consumes the device for other uses.
            // For each interface: call WickedInterface::new(name), configure it and add that to
            // wicked_interfaces Vec
            if let NetworkDeviceV1::BondDevice(b) = config {
                for device in &b.interfaces {
                    let mut wicked_sub_interface = WickedInterface::new(device.clone());
                    wicked_sub_interface.link = Some(WickedLinkConfig {
                        master: name.clone(),
                    });

                    wicked_interfaces.push(wicked_sub_interface)
                }
            }

            wicked_interfaces.push(interface)
        }

        wicked_interfaces
    }
}

#[allow(clippy::to_string_in_format_args)]
impl Validate for NetConfigV3 {
    fn validate(&self) -> Result<()> {
        // Create HashSet of known device names for checking duplicates
        let mut interface_names: HashSet<&InterfaceName> = self.net_devices.keys().collect();
        for (_name, device) in &self.net_devices {
            if let NetworkDeviceV1::VlanDevice(vlan) = device {
                // It is valid to stack more than one vlan on a single device, but we need them all
                // for checking bonds which can't share devices.
                interface_names.insert(&vlan.device);
            }
        }

        for (name, device) in &self.net_devices {
            // Bonds create the interfaces automatically, specifying those interfaces would cause a
            // collision so this emits an error for any that are found
            if let NetworkDeviceV1::BondDevice(config) = device {
                for interface in &config.interfaces {
                    // This checks if the insert already found one, which would be a failure
                    if !interface_names.insert(interface) {
                        return error::InvalidNetConfigSnafu {
                            reason: format!(
                                "{} in bond {} cannot be manually configured",
                                interface.to_string(),
                                name.to_string()
                            ),
                        }
                        .fail();
                    }
                }
            }
            device.validate()?;
        }

        let primary_count = self
            .net_devices
            .values()
            .filter(|v| v.primary() == Some(true))
            .count();
        ensure!(
            primary_count <= 1,
            error::InvalidNetConfigSnafu {
                reason: "multiple primary interfaces defined, expected 1"
            }
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::net_config::test_macros::{
        basic_tests, bonding_tests, dhcp_tests, static_address_tests, vlan_tests,
    };

    basic_tests!(3);
    dhcp_tests!(3);
    static_address_tests!(3);
    vlan_tests!(3);
    bonding_tests!(3);
}
