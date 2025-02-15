mod descriptor;
mod mnemonic;

pub use descriptor::{
    BackupDescriptor, DefineDescriptor, ImportDescriptor, ParticipateXpub, RegisterDescriptor,
};

pub use mnemonic::{BackupMnemonic, RecoverMnemonic};

use std::path::PathBuf;
use std::str::FromStr;

use iced::Command;
use liana::{
    config::BitcoindConfig,
    miniscript::bitcoin::{util::bip32::Fingerprint, Network},
};

use jsonrpc::{client::Client, simple_http::SimpleHttpTransport};

use liana_ui::{component::form, widget::*};

use crate::installer::{
    context::Context,
    message::{self, Message},
    view, Error,
};

pub trait Step {
    fn update(&mut self, _message: Message) -> Command<Message> {
        Command::none()
    }
    fn view(&self, progress: (usize, usize)) -> Element<Message>;
    fn load_context(&mut self, _ctx: &Context) {}
    fn load(&self) -> Command<Message> {
        Command::none()
    }
    fn skip(&self, _ctx: &Context) -> bool {
        false
    }
    fn apply(&mut self, _ctx: &mut Context) -> bool {
        true
    }
}

#[derive(Default)]
pub struct Welcome {}

impl Step for Welcome {
    fn view(&self, _progress: (usize, usize)) -> Element<Message> {
        view::welcome()
    }
}

impl From<Welcome> for Box<dyn Step> {
    fn from(s: Welcome) -> Box<dyn Step> {
        Box::new(s)
    }
}

pub struct DefineBitcoind {
    cookie_path: form::Value<String>,
    address: form::Value<String>,
    is_running: Option<Result<(), Error>>,
}

fn bitcoind_default_cookie_path(network: &Network) -> Option<String> {
    #[cfg(target_os = "linux")]
    let configs_dir = dirs::home_dir();

    #[cfg(not(target_os = "linux"))]
    let configs_dir = dirs::config_dir();

    if let Some(mut path) = configs_dir {
        #[cfg(target_os = "linux")]
        path.push(".bitcoin");

        #[cfg(not(target_os = "linux"))]
        path.push("Bitcoin");

        match network {
            Network::Bitcoin => {
                path.push(".cookie");
            }
            Network::Testnet => {
                path.push("testnet3/.cookie");
            }
            Network::Regtest => {
                path.push("regtest/.cookie");
            }
            Network::Signet => {
                path.push("signet/.cookie");
            }
        }

        return path.to_str().map(|s| s.to_string());
    }
    None
}

fn bitcoind_default_address(network: &Network) -> String {
    match network {
        Network::Bitcoin => "127.0.0.1:8332".to_string(),
        Network::Testnet => "127.0.0.1:18332".to_string(),
        Network::Regtest => "127.0.0.1:18443".to_string(),
        Network::Signet => "127.0.0.1:38332".to_string(),
    }
}

impl DefineBitcoind {
    pub fn new() -> Self {
        Self {
            cookie_path: form::Value::default(),
            address: form::Value::default(),
            is_running: None,
        }
    }

    pub fn ping(&self) -> Command<Message> {
        let address = self.address.value.to_owned();
        let cookie_path = self.cookie_path.value.to_owned();
        Command::perform(
            async move {
                let cookie = std::fs::read_to_string(&cookie_path)
                    .map_err(|e| Error::Bitcoind(format!("Failed to read cookie file: {}", e)))?;
                let client = Client::with_transport(
                    SimpleHttpTransport::builder()
                        .url(&address)?
                        .timeout(std::time::Duration::from_secs(3))
                        .cookie_auth(cookie)
                        .build(),
                );
                client.send_request(client.build_request("echo", &[]))?;
                Ok(())
            },
            |res| Message::DefineBitcoind(message::DefineBitcoind::PingBitcoindResult(res)),
        )
    }
}

impl Step for DefineBitcoind {
    fn load_context(&mut self, ctx: &Context) {
        if self.cookie_path.value.is_empty() {
            self.cookie_path.value =
                bitcoind_default_cookie_path(&ctx.bitcoin_config.network).unwrap_or_default()
        }
        if self.address.value.is_empty() {
            self.address.value = bitcoind_default_address(&ctx.bitcoin_config.network);
        }
    }
    fn update(&mut self, message: Message) -> Command<Message> {
        if let Message::DefineBitcoind(msg) = message {
            match msg {
                message::DefineBitcoind::PingBitcoind => {
                    self.is_running = None;
                    return self.ping();
                }
                message::DefineBitcoind::PingBitcoindResult(res) => self.is_running = Some(res),
                message::DefineBitcoind::AddressEdited(address) => {
                    self.is_running = None;
                    self.address.value = address;
                    self.address.valid = true;
                }
                message::DefineBitcoind::CookiePathEdited(path) => {
                    self.is_running = None;
                    self.cookie_path.value = path;
                    self.address.valid = true;
                }
            };
        };
        Command::none()
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        match (
            PathBuf::from_str(&self.cookie_path.value),
            std::net::SocketAddr::from_str(&self.address.value),
        ) {
            (Err(_), Ok(_)) => {
                self.cookie_path.valid = false;
                false
            }
            (Ok(_), Err(_)) => {
                self.address.valid = false;
                false
            }
            (Err(_), Err(_)) => {
                self.cookie_path.valid = false;
                self.address.valid = false;
                false
            }
            (Ok(path), Ok(addr)) => {
                ctx.bitcoind_config = Some(BitcoindConfig {
                    cookie_path: path,
                    addr,
                });
                true
            }
        }
    }

    fn view(&self, progress: (usize, usize)) -> Element<Message> {
        view::define_bitcoin(
            progress,
            &self.address,
            &self.cookie_path,
            self.is_running.as_ref(),
        )
    }

    fn load(&self) -> Command<Message> {
        self.ping()
    }
}

impl Default for DefineBitcoind {
    fn default() -> Self {
        Self::new()
    }
}

impl From<DefineBitcoind> for Box<dyn Step> {
    fn from(s: DefineBitcoind) -> Box<dyn Step> {
        Box::new(s)
    }
}

pub struct Final {
    generating: bool,
    context: Option<Context>,
    warning: Option<String>,
    config_path: Option<PathBuf>,
    hot_signer_fingerprint: Fingerprint,
    hot_signer_is_not_used: bool,
}

impl Final {
    pub fn new(hot_signer_fingerprint: Fingerprint) -> Self {
        Self {
            context: None,
            generating: false,
            warning: None,
            config_path: None,
            hot_signer_fingerprint,
            hot_signer_is_not_used: false,
        }
    }
}

impl Step for Final {
    fn load_context(&mut self, ctx: &Context) {
        self.context = Some(ctx.clone());
        if let Some(signer) = &ctx.recovered_signer {
            self.hot_signer_fingerprint = signer.fingerprint();
            self.hot_signer_is_not_used = false;
        } else if ctx
            .descriptor
            .as_ref()
            .unwrap()
            .to_string()
            .contains(&self.hot_signer_fingerprint.to_string())
        {
            self.hot_signer_is_not_used = false;
        } else {
            self.hot_signer_is_not_used = true;
        }
    }
    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Installed(res) => {
                self.generating = false;
                match res {
                    Err(e) => {
                        self.config_path = None;
                        self.warning = Some(e.to_string());
                    }
                    Ok(path) => self.config_path = Some(path),
                }
            }
            Message::Install => {
                self.generating = true;
                self.config_path = None;
                self.warning = None;
            }
            _ => {}
        };
        Command::none()
    }

    fn view(&self, progress: (usize, usize)) -> Element<Message> {
        let ctx = self.context.as_ref().unwrap();
        let desc = ctx.descriptor.as_ref().unwrap().to_string();
        view::install(
            progress,
            ctx,
            desc,
            self.generating,
            self.config_path.as_ref(),
            self.warning.as_ref(),
            if self.hot_signer_is_not_used {
                None
            } else {
                Some(self.hot_signer_fingerprint)
            },
        )
    }
}

impl From<Final> for Box<dyn Step> {
    fn from(s: Final) -> Box<dyn Step> {
        Box::new(s)
    }
}
