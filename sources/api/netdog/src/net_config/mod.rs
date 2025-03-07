//! The net_config module contains the structures needed to deserialize a `net.toml` file.  It also
//! includes contains the `FromStr` implementations to create a `NetConfig` from string, like from
//! the kernel command line.
//!
//! These structures are the user-facing options for configuring one or more network interfaces.

pub(crate) mod devices;
mod dhcp;
mod error;
mod static_address;
mod v1;
mod v2;
mod v3;

use crate::wicked::WickedInterface;
pub(crate) use dhcp::{Dhcp4ConfigV1, Dhcp4OptionsV1, Dhcp6ConfigV1, Dhcp6OptionsV1};
pub(crate) use error::{Error, Result};
use serde::Deserialize;
use snafu::{ensure, ResultExt};
pub(crate) use static_address::{RouteTo, RouteV1, StaticConfigV1};
use std::fs;
use std::path::Path;
use std::str::FromStr;
pub(crate) use v1::NetConfigV1;

static DEFAULT_INTERFACE_PREFIX: &str = "netdog.default-interface=";

/// This trait must be implemented by each new version of network config
pub(crate) trait Interfaces {
    /// Returns the primary network interface.
    fn primary_interface(&self) -> Option<String>;

    /// Does the config contain any interfaces?
    fn has_interfaces(&self) -> bool;

    /// Converts the network config into a list of `WickedInterface` structs, suitable for writing
    /// to file
    fn as_wicked_interfaces(&self) -> Vec<WickedInterface>;
}

impl<I: Interfaces> Interfaces for Box<I> {
    fn primary_interface(&self) -> Option<String> {
        (**self).primary_interface()
    }

    fn has_interfaces(&self) -> bool {
        (**self).has_interfaces()
    }

    fn as_wicked_interfaces(&self) -> Vec<WickedInterface> {
        (**self).as_wicked_interfaces()
    }
}

/// This private trait must also be implemented by each new version of network config.  It is used
/// during the deserialization of the config to validate the configuration, ensuring there are no
/// conflicting options set, etc.
trait Validate {
    /// Validate the network configuration
    fn validate(&self) -> Result<()>;
}

impl<V: Validate> Validate for Box<V> {
    fn validate(&self) -> Result<()> {
        (**self).validate()
    }
}

/// Read the network config from file, returning an object that implements the `Interfaces` trait
pub(crate) fn from_path<P>(path: P) -> Result<Option<Box<dyn Interfaces>>>
where
    P: AsRef<Path>,
{
    let path = path.as_ref();
    let net_config_str =
        fs::read_to_string(path).context(error::NetConfigReadFailedSnafu { path })?;
    let net_config = deserialize_config(&net_config_str)?;

    if !net_config.has_interfaces() {
        return Ok(None);
    }

    Ok(Some(net_config))
}

/// Deserialize the network config, using the version key to determine which config struct to
/// deserialize into
fn deserialize_config(config_str: &str) -> Result<Box<dyn Interfaces>> {
    #[derive(Debug, Deserialize)]
    struct ConfigToml {
        version: u8,
        #[serde(flatten)]
        interface_config: toml::Value,
    }

    let ConfigToml {
        version,
        interface_config,
    } = toml::from_str(config_str).context(error::NetConfigParseSnafu)?;

    let net_config: Box<dyn Interfaces> = match version {
        1 => validate_config::<v1::NetConfigV1>(interface_config)?,
        2 => validate_config::<v2::NetConfigV2>(interface_config)?,
        3 => validate_config::<v3::NetConfigV3>(interface_config)?,
        _ => {
            return error::InvalidNetConfigSnafu {
                reason: format!("Unknown network config version: {}", version),
            }
            .fail();
        }
    };

    Ok(net_config)
}

fn validate_config<'a, I>(config_value: toml::Value) -> Result<Box<I>>
where
    I: Interfaces + Validate + Deserialize<'a>,
{
    let config = config_value
        .try_into::<I>()
        .context(error::NetConfigParseSnafu)?;
    config.validate()?;

    Ok(Box::new(config))
}

/// Read a network config from the kernel command line
pub(crate) fn from_command_line<P>(path: P) -> Result<Option<Box<dyn Interfaces>>>
where
    P: AsRef<Path>,
{
    let p = path.as_ref();
    let kernel_cmdline =
        fs::read_to_string(p).context(error::KernelCmdlineReadFailedSnafu { path: p })?;

    let mut maybe_interfaces = kernel_cmdline
        .split_whitespace()
        .filter(|s| s.starts_with(DEFAULT_INTERFACE_PREFIX));

    let default_interface = match maybe_interfaces.next() {
        Some(interface_str) => interface_str
            .trim_start_matches(DEFAULT_INTERFACE_PREFIX)
            .to_string(),
        None => return Ok(None),
    };

    ensure!(
        maybe_interfaces.next().is_none(),
        error::MultipleDefaultInterfacesSnafu
    );

    let net_config = NetConfigV1::from_str(&default_interface)?;
    Ok(Some(Box::new(net_config)))
}

#[cfg(test)]
mod test_macros;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn test_data() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data")
    }

    fn cmdline() -> PathBuf {
        test_data().join("cmdline")
    }

    fn net_config() -> PathBuf {
        test_data().join("net_config")
    }

    #[test]
    fn ok_net_config() {
        let ok = net_config().join("net_config.toml");
        assert!(from_path(ok).unwrap().is_some())
    }

    #[test]
    fn no_interfaces_net_config() {
        let bad = net_config().join("no_interfaces.toml");
        assert!(from_path(bad).unwrap().is_none())
    }

    #[test]
    fn ok_cmdline() {
        let cmdline = cmdline().join("ok");
        assert!(from_command_line(cmdline).unwrap().is_some());
    }

    #[test]
    fn multiple_interface_from_cmdline() {
        let cmdline = cmdline().join("multiple_interface");
        assert!(from_command_line(cmdline).is_err())
    }

    #[test]
    fn no_interfaces_cmdline() {
        let cmdline = cmdline().join("no_interfaces");
        assert!(from_command_line(cmdline).unwrap().is_none())
    }
}
