//! The transaction layer: routing messages to transactions, and cleaning up after them.

use std::collections::HashMap;

use crate::message::{Message, Method, Request, Response};
use crate::transaction::client::{ClientState, ClientTransaction};
use crate::transaction::key::TransactionKey;
use crate::transaction::server::{ServerState, ServerTransaction};
use crate::transaction::timing::{Timer, Timers};
use crate::transaction::{Output, Reliability, TuEvent};

/// Where a message went.
#[derive(Debug)]
pub enum Dispatch {
    /// It matched a transaction, which produced these outputs.
    Matched {
        /// The transaction it matched.
        key: TransactionKey,
        /// What the transaction wants done.
        outputs: Vec<Output>,
    },
    /// It created a new server transaction.
    Created {
        /// The new transaction's key.
        key: TransactionKey,
        /// What the transaction wants done.
        outputs: Vec<Output>,
    },
    /// It matched nothing.
    ///
    /// Passed up rather than dropped. An unmatched response may be a stray fork answer the
    /// core has no business discarding silently, and an unmatched ACK for a 2xx is normal.
    Unmatched(Box<Message>),
}

/// Holds the transactions in flight and routes messages to them.
///
/// Sans-IO like everything beneath it: the driver feeds messages and fired timers in, and
/// performs the outputs.
#[derive(Debug)]
pub struct TransactionLayer {
    client: HashMap<TransactionKey, ClientTransaction>,
    server: HashMap<TransactionKey, ServerTransaction>,
    timers: Timers,
}

impl TransactionLayer {
    /// A layer with the given timer constants.
    #[must_use]
    pub fn new(timers: Timers) -> Self {
        Self {
            client: HashMap::new(),
            server: HashMap::new(),
            timers,
        }
    }

    /// How many transactions are in flight, as (client, server).
    ///
    /// Exposed because a transaction store that leaks is a slow, quiet outage, and a test
    /// that asserts on this is the cheapest way to notice.
    #[must_use]
    pub fn len(&self) -> (usize, usize) {
        (self.client.len(), self.server.len())
    }

    /// Whether no transactions are in flight.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.client.is_empty() && self.server.is_empty()
    }

    /// Send a request, creating a client transaction for it.
    pub fn send_request(
        &mut self,
        request: Request,
        reliability: Reliability,
    ) -> Option<(TransactionKey, Vec<Output>)> {
        let key = TransactionKey::from_sent_request(&request)?;
        let (tx, outputs) = ClientTransaction::new(request, reliability, self.timers);
        self.client.insert(key.clone(), tx);
        Some((key, outputs))
    }

    /// Route an incoming message.
    pub fn receive(&mut self, message: Message, reliability: Reliability) -> Dispatch {
        match message {
            Message::Request(request) => self.receive_request(request, reliability),
            Message::Response(response) => self.receive_response(response),
        }
    }

    fn receive_request(&mut self, request: Request, reliability: Reliability) -> Dispatch {
        let Some(key) = TransactionKey::from_request(&request) else {
            return Dispatch::Unmatched(Box::new(Message::Request(request)));
        };

        if let Some(tx) = self.server.get_mut(&key) {
            let outputs = tx.on_request(&request);
            let terminated = tx.state().is_terminated();
            if terminated {
                self.server.remove(&key);
            }
            return Dispatch::Matched { key, outputs };
        }

        // An ACK that matches no transaction is an ACK for a 2xx whose transaction has already
        // gone. That is ordinary, and it belongs to the transaction user.
        if request.method == Method::Ack {
            return Dispatch::Unmatched(Box::new(Message::Request(request)));
        }

        let (tx, outputs) = ServerTransaction::new(request, reliability, self.timers);
        self.server.insert(key.clone(), tx);
        Dispatch::Created { key, outputs }
    }

    fn receive_response(&mut self, response: Response) -> Dispatch {
        let Some(key) = TransactionKey::from_response(&response) else {
            return Dispatch::Unmatched(Box::new(Message::Response(response)));
        };

        let Some(tx) = self.client.get_mut(&key) else {
            return Dispatch::Unmatched(Box::new(Message::Response(response)));
        };

        let outputs = tx.on_response(response);
        if tx.state().is_terminated() {
            self.client.remove(&key);
        }
        Dispatch::Matched { key, outputs }
    }

    /// Send a response from the transaction user.
    pub fn send_response(&mut self, key: &TransactionKey, response: Response) -> Vec<Output> {
        let Some(tx) = self.server.get_mut(key) else {
            return Vec::new();
        };
        let outputs = tx.on_tu_response(response);
        if tx.state().is_terminated() {
            self.server.remove(key);
        }
        outputs
    }

    /// A timer fired for a transaction.
    pub fn on_timer(&mut self, key: &TransactionKey, timer: Timer) -> Vec<Output> {
        if let Some(tx) = self.client.get_mut(key) {
            let outputs = tx.on_timer(timer);
            if tx.state().is_terminated() {
                self.client.remove(key);
            }
            return outputs;
        }
        if let Some(tx) = self.server.get_mut(key) {
            let outputs = tx.on_timer(timer);
            if tx.state().is_terminated() {
                self.server.remove(key);
            }
            return outputs;
        }
        Vec::new()
    }

    /// The transport failed for a transaction.
    pub fn on_transport_error(&mut self, key: &TransactionKey) -> Vec<Output> {
        if let Some(tx) = self.client.get_mut(key) {
            let outputs = tx.on_transport_error();
            self.client.remove(key);
            return outputs;
        }
        if let Some(tx) = self.server.get_mut(key) {
            let outputs = tx.on_transport_error();
            self.server.remove(key);
            return outputs;
        }
        Vec::new()
    }

    /// The state of a client transaction, if it exists.
    #[must_use]
    pub fn client_state(&self, key: &TransactionKey) -> Option<ClientState> {
        self.client.get(key).map(ClientTransaction::state)
    }

    /// The state of a server transaction, if it exists.
    #[must_use]
    pub fn server_state(&self, key: &TransactionKey) -> Option<ServerState> {
        self.server.get(key).map(ServerTransaction::state)
    }
}

/// Convenience: pull the transaction-user events out of a batch of outputs.
#[must_use]
pub fn tu_events(outputs: &[Output]) -> Vec<&TuEvent> {
    outputs
        .iter()
        .filter_map(|o| match o {
            Output::ToTu(event) => Some(event.as_ref()),
            _ => None,
        })
        .collect()
}

/// Convenience: pull the messages to send out of a batch of outputs.
#[must_use]
pub fn sent_messages(outputs: &[Output]) -> Vec<&Message> {
    outputs
        .iter()
        .filter_map(|o| match o {
            Output::Send(message) => Some(message.as_ref()),
            _ => None,
        })
        .collect()
}
