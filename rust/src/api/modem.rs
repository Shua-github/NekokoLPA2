//! The modem's card, reached over the bus the modem itself publishes.
//!
//! One session is one open client, and there is one way to open it here: the
//! QRTR bus, through a socket the Dart side holds. The app may not open that
//! socket — SELinux refuses it one of its own and refuses it the use of one
//! another process opened — so the socket lives in the process Shizuku runs
//! and everything above it asks Dart to move a datagram for it.

use core::panic::UnwindSafe;
use core::time::Duration;

use flutter_rust_bridge::{DartFnFuture, frb};
use rm_client_core::{LocalUim, SlotState};
use rm_qmi::{LocalQrtrSocket, QrtrAddress, QrtrTransport};

/// A QMI client over the QRTR socket, which is in the process Shizuku runs.
type QrtrQmi = rm_qmi::QmiClient<QrtrTransport<DartQrtrSocket>>;

/// One card the modem sees.
pub struct ModemSlot {
    /// Slot number, counted from one.
    pub slot: u8,
    pub present: bool,
    pub ready: bool,
    pub imei: Option<String>,
    pub atr: Option<Vec<u8>>,
}

/// Where a datagram is going, or came from: a node and a port on the bus.
pub struct QrtrSocketAddress {
    pub node: u32,
    pub port: u32,
}

/// One datagram on the bus.
pub struct QrtrSocketDatagram {
    /// Address it is for, or came from.
    pub node: u32,
    pub port: u32,
    pub data: Vec<u8>,
}

/// A card reached over the modem's bus.
///
/// The app opens a session when it connects the reader and drops it when it
/// lets the reader go; dropping it releases whatever the session claimed.
#[frb(opaque)]
pub struct ModemSession {
    client: QrtrQmi,
}

impl ModemSession {
    pub async fn state(&mut self) -> Result<Vec<ModemSlot>, String> {
        let states = self.client.state().await.map_err(describe)?;

        Ok(states
            .into_iter()
            .map(|(slot, state)| ModemSlot::new(slot, state))
            .collect())
    }

    pub async fn reset(&mut self, slot: u8) -> Result<Vec<u8>, String> {
        self.client.reset(slot).await.map_err(describe)
    }

    pub async fn open_channel(&mut self, slot: u8, aid: Vec<u8>) -> Result<u8, String> {
        self.client.open_channel(slot, &aid).await.map_err(describe)
    }

    pub async fn transmit(
        &mut self,
        slot: u8,
        channel: u8,
        apdu: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.client
            .transmit(slot, channel, &apdu)
            .await
            .map_err(describe)
    }

    pub async fn close_channel(&mut self, slot: u8, channel: u8) -> Result<(), String> {
        self.client
            .close_channel(slot, channel)
            .await
            .map_err(describe)
    }
}

impl ModemSlot {
    fn new(slot: u8, state: SlotState) -> Self {
        Self {
            slot,
            present: state.present,
            ready: state.ready,
            imei: state.imei,
            atr: state.atr,
        }
    }
}

/// Open a QMI session over the QRTR bus, through a socket Dart holds.
///
/// The app may not open a socket on the bus: SELinux refuses it one of its own,
/// and refuses it the use of one another process opened. The socket therefore
/// stays in the process Shizuku runs, and these three calls are the whole of
/// what the transport needs from it — where the socket is addressed, send this
/// datagram there, wait for one to come back — which is what the Dart side
/// implements over the channel to that process.
pub async fn open_qmi_qrtr_session(
    address: impl Fn() -> DartFnFuture<Result<QrtrSocketAddress, anyhow::Error>> + UnwindSafe + 'static,
    send: impl Fn(QrtrSocketDatagram) -> DartFnFuture<Result<(), anyhow::Error>> + UnwindSafe + 'static,
    receive: impl Fn(u32) -> DartFnFuture<Result<Option<QrtrSocketDatagram>, anyhow::Error>>
        + UnwindSafe
        + 'static,
) -> Result<ModemSession, String> {
    let socket = DartQrtrSocket {
        address: DartCall(Box::new(address)),
        send: DartCall(Box::new(send)),
        receive: DartCall(Box::new(receive)),
    };

    Ok(ModemSession {
        client: rm_qmi::QmiClient::new(QrtrTransport::new(socket)),
    })
}

/// A socket the Dart side implements, because the bus socket is not in this
/// process.
struct DartQrtrSocket {
    address: DartCall<Box<dyn Fn() -> DartFnFuture<Result<QrtrSocketAddress, anyhow::Error>>>>,
    send: DartCall<Box<dyn Fn(QrtrSocketDatagram) -> DartFnFuture<Result<(), anyhow::Error>>>>,
    receive: DartCall<
        Box<dyn Fn(u32) -> DartFnFuture<Result<Option<QrtrSocketDatagram>, anyhow::Error>>>,
    >,
}

/// One call into Dart.
///
/// The transport makes its calls from whichever thread drives the session, and
/// the bridge under them hands the arguments to Dart's own thread and waits
/// for the answer, which is what makes them safe to move and to share. The
/// closure the generated binding hands over does not say that of itself, so
/// this says it instead.
struct DartCall<F>(F);

// SAFETY: a call carries nothing but the bridge to Dart, which is thread safe.
unsafe impl<F> Send for DartCall<F> {}
unsafe impl<F> Sync for DartCall<F> {}

/// What the Dart side says when a socket call did not work.
#[derive(Debug)]
pub struct QrtrSocketError(String);

impl core::fmt::Display for QrtrSocketError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl core::error::Error for QrtrSocketError {}

impl LocalQrtrSocket for DartQrtrSocket {
    type Error = QrtrSocketError;

    async fn address(&mut self) -> Result<QrtrAddress, Self::Error> {
        let address = (self.address.0)()
            .await
            .map_err(|error| QrtrSocketError(error.to_string()))?;

        Ok(QrtrAddress {
            node: address.node,
            port: address.port,
        })
    }

    async fn send(&mut self, to: QrtrAddress, datagram: &[u8]) -> Result<(), Self::Error> {
        (self.send.0)(QrtrSocketDatagram {
            node: to.node,
            port: to.port,
            data: datagram.to_vec(),
        })
        .await
        .map_err(|error| QrtrSocketError(error.to_string()))
    }

    async fn receive(
        &mut self,
        buffer: &mut [u8],
        timeout: Duration,
    ) -> Result<Option<(usize, QrtrAddress)>, Self::Error> {
        let milliseconds = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);

        let Some(datagram) = (self.receive.0)(milliseconds)
            .await
            .map_err(|error| QrtrSocketError(error.to_string()))?
        else {
            return Ok(None);
        };

        let copied = datagram.data.len().min(buffer.len());
        buffer[..copied].copy_from_slice(&datagram.data[..copied]);

        Ok(Some((
            copied,
            QrtrAddress {
                node: datagram.node,
                port: datagram.port,
            },
        )))
    }
}

/// The message a step of the card failed with, as the app shows it.
fn describe<E: core::fmt::Display>(error: E) -> String {
    error.to_string()
}
