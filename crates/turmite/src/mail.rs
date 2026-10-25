//! Outgoing email over SMTP. Without `TURMITE_SMTP_URL`, messages go to the log (testnet only).

use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
};

use crate::{
    config::{Mode, Smtp},
    error::{Error, Result},
};

#[derive(Clone)]
pub struct Mailer {
    transport: Option<AsyncSmtpTransport<Tokio1Executor>>,
    from: Option<Mailbox>,
    mode: Mode,
}

impl Mailer {
    pub fn new(smtp: Option<&Smtp>, mode: Mode) -> Result<Self> {
        let Some(smtp) = smtp else {
            return Ok(Self { transport: None, from: None, mode });
        };
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(&smtp.url)
            .map_err(|e| Error::BadRequest(format!("TURMITE_SMTP_URL: {e}")))?
            .build();
        let from = smtp.from.parse().map_err(|e| Error::BadRequest(format!("TURMITE_SMTP_FROM: {e}")))?;
        Ok(Self { transport: Some(transport), from: Some(from), mode })
    }

    pub fn is_configured(&self) -> bool {
        self.transport.is_some()
    }

    /// Send a plain-text message. On testnet without SMTP the message is logged instead.
    pub async fn send(&self, to: &str, subject: &str, body: &str) -> Result<()> {
        let (Some(transport), Some(from)) = (&self.transport, &self.from) else {
            if self.mode.is_mainnet() {
                return Err(Error::BadRequest("Email is not set up. Set TURMITE_SMTP_URL.".into()));
            }
            tracing::warn!(%to, %subject, %body, "smtp not set up; email written to the log");
            return Ok(());
        };
        let to: Mailbox = to.parse().map_err(|e| Error::BadRequest(format!("bad email address: {e}")))?;
        let message = Message::builder()
            .from(from.clone())
            .to(to)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())
            .map_err(|e| Error::Internal(format!("build email: {e}")))?;
        transport.send(message).await.map(|_| ()).map_err(|e| Error::Internal(format!("send email: {e}")))
    }
}
