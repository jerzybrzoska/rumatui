use std::collections::HashMap;
use std::fmt;
use std::time::Duration;
use std::sync::Arc;

use anyhow::{Context, Result};
use matrix_sdk::{
    self,
    api::r0::message::create_message_event,
    api::r0::message::get_message_events,
    events::room::message::MessageEventContent,
    identifiers::{RoomId, UserId},
    AsyncClient, AsyncClientConfig, Room, SyncSettings, Client as BaseClient,
};
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use url::Url;

pub mod client_loop;
pub mod event_stream;


const SYNC_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct MatrixClient {
    pub inner: AsyncClient,
    homeserver: String,
    user: Option<UserId>,
    settings: SyncSettings,
    next_batch: Option<String>,
    last_scroll: Option<String>,
}
unsafe impl Send for MatrixClient {}

impl fmt::Debug for MatrixClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MatrixClient")
            .field("user", &self.user)
            .finish()
    }
}

impl MatrixClient {
    pub fn new(homeserver: &str) -> Result<Self, failure::Error> {
        let _client_config = AsyncClientConfig::default();
        let homeserver_url = Url::parse(&homeserver)?;

        let client = Self {
            inner: AsyncClient::new(homeserver_url, None)?,
            homeserver: homeserver.into(),
            user: None,
            settings: SyncSettings::default(),
            next_batch: None,
            last_scroll: None,
        };

        Ok(client)
    }

    /// Returns an Arc of the BaseClient
    pub fn base_client(&self) -> Arc<RwLock<BaseClient>> {
        self.inner.base_client()
    }

    pub fn sync_token(&self) -> Option<String> {
        self.next_batch.clone()
    }

    pub(crate) async fn login(
        &mut self,
        username: String,
        password: String,
    ) -> Result<HashMap<RoomId, Arc<Mutex<Room>>>> {
        let res = self.inner.login(username, password, None, None).await?;
        self.user = Some(res.user_id.clone());

        let _response = self.inner.sync(SyncSettings::default().timeout(SYNC_TIMEOUT)).await?;

        Ok(self.inner.get_rooms().await)
    }

    pub(crate) async fn sync(&mut self) -> Result<()> {
        self.next_batch = self.inner.sync_token().await;
        let tkn = self.next_batch.as_ref().unwrap();
        self.settings = SyncSettings::new()
            .token(tkn)
            .full_state(true)
            .timeout(SYNC_TIMEOUT);
        self.inner
            .sync(self.settings.to_owned())
            .await
            .map(|res| ())
            .map_err(|e| anyhow::Error::from(e))
    }

    /// Sends a MessageEvent to the specified room.
    ///
    /// # Arguments
    ///
    /// * id - A valid RoomId otherwise sending will fail.
    /// * msg - `MessageEventContent`s is an enum that can handle all the types
    /// of messages eg. `Text`, `Audio`, `Video` ect.
    pub(crate) async fn send_message(
        &mut self,
        id: &RoomId,
        msg: MessageEventContent,
    ) -> Result<create_message_event::Response> {
        self.inner
            .room_send(&id, msg)
            .await
            .context("Message failed to send")
    }

    /// Gets the `RoomEvent`s backwards in time, when user scrolls up.
    ///
    /// This uses the current sync token to look backwards from that point.
    /// 
    /// # Arguments
    ///
    /// * id - A valid RoomId otherwise sending will fail.
    /// 
    pub(crate) async fn get_messages(
        &mut self,
        id: &RoomId,
    ) -> Result<get_message_events::IncomingResponse> {
        let from = if let Some(scroll) = &self.last_scroll {
            scroll.clone()
        } else {
            self.next_batch.as_ref().unwrap().clone()
        };
        let request = get_message_events::Request {
            room_id: id.clone(),
            from,
            to: None,
            dir: get_message_events::Direction::Backward,
            limit: js_int::UInt::new(20),
            filter: None,
        };

        match self.inner.send(request).await.map_err(|e| anyhow::Error::from(e)) {
            Ok(res) => {
                self.last_scroll = Some(res.end.clone());
                Ok(res)
            },
            Err(err) => Err(err),
        }
    }
}
