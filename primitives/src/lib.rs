// Copyright (C) Polytope Labs Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Traits and types required to compose the tesseract relayer
pub mod config;
pub mod queue;

use anyhow::anyhow;
use futures::Stream;
pub use ismp::events::StateMachineUpdated;
use ismp::{
	consensus::{ConsensusStateId, StateMachineHeight, StateMachineId},
	events::Event,
	host::StateMachine,
	messaging::{ConsensusMessage, Message},
	router::Get,
};
use primitive_types::H256;
use std::{pin::Pin, sync::Arc, time::Duration};

/// Provides an interface for accessing new events and ISMP data on the chain which must be
/// relayed to the counterparty chain.

#[derive(Copy, Clone, Debug)]
pub struct Query {
	pub source_chain: StateMachine,
	pub dest_chain: StateMachine,
	pub nonce: u64,
	pub commitment: H256,
}

#[derive(Copy, Clone, Debug)]
pub struct ChallengePeriodStarted {
	/// State machine update still in challenge period
	pub state_machine_height: StateMachineHeight,
	/// Height at which this event was emitted on the host chain
	pub host_chain_height: u64,
}

/// Stream alias
pub type BoxStream<I> = Pin<Box<dyn Stream<Item = Result<I, anyhow::Error>> + Send>>;

#[async_trait::async_trait]
pub trait IsmpProvider: Reconnect {
	/// Query the latest consensus state of a client
	async fn query_consensus_state(
		&self,
		at: Option<u64>,
		id: ConsensusStateId,
	) -> Result<Vec<u8>, anyhow::Error>;

	/// Query the latest height at which some state machine was last updated
	async fn query_latest_height(&self, id: StateMachineId) -> Result<u32, anyhow::Error>;

	/// Query the height at which a message was last processed for the provided state machine
	async fn query_latest_messaging_height(&self, id: StateMachineId)
		-> Result<u64, anyhow::Error>;

	/// Query the timestamp at which the client was last updated
	async fn query_consensus_update_time(
		&self,
		id: ConsensusStateId,
	) -> Result<Duration, anyhow::Error>;

	/// Query the challenge period for client
	async fn query_challenge_period(&self, id: ConsensusStateId)
		-> Result<Duration, anyhow::Error>;

	/// Query the latest timestamp for chain
	async fn query_timestamp(&self) -> Result<Duration, anyhow::Error>;

	/// Query a requests proof
	/// Return the scale encoded proof
	async fn query_requests_proof(
		&self,
		at: u64,
		keys: Vec<Query>,
	) -> Result<Vec<u8>, anyhow::Error>;

	/// Query a responses proof
	/// Return the scale encoded proof
	async fn query_responses_proof(
		&self,
		at: u64,
		keys: Vec<Query>,
	) -> Result<Vec<u8>, anyhow::Error>;

	/// Query state proof for some keys, return scaled encoded proof
	async fn query_state_proof(
		&self,
		at: u64,
		keys: Vec<Vec<u8>>,
	) -> Result<Vec<u8>, anyhow::Error>;

	/// Query all ismp events on host that can be processed for a [`StateMachineUpdated`]
	/// event on the counterparty
	async fn query_ismp_events(
		&self,
		event: StateMachineUpdated,
	) -> Result<Vec<Event>, anyhow::Error>;

	/// Query requests
	async fn query_pending_get_requests(&self, height: u64) -> Result<Vec<Get>, anyhow::Error>;

	/// Name of this chain, used in logs.
	fn name(&self) -> String;

	/// State Machine Id for this client which would be it's state machine id
	/// on the counterparty chain
	fn state_machine_id(&self) -> StateMachineId;

	/// Should return a numerical value for the max gas allowed for transactions in a block.
	fn block_max_gas(&self) -> u64;

	/// Should return a numerical estimate of the gas to be consumed for a batch of messages.
	async fn estimate_gas(&self, msg: Vec<Message>) -> Result<u64, anyhow::Error>;

	/// Return a stream that watches for updates to [`counterparty_state_id`], yields when new
	/// [`StateMachineUpdated`] event is observed for [`counterparty_state_id`]
	async fn state_machine_update_notification(
		&self,
		counterparty_state_id: StateMachineId,
	) -> BoxStream<StateMachineUpdated>;

	/// This should be used to submit new messages [`Vec<Message>`] from a counterparty chain to
	/// this chain.
	///
	/// Should only return Ok if the transaction was successfully inserted into a block.
	async fn submit(&self, messages: Vec<Message>) -> Result<(), anyhow::Error>;
}

/// Provides an interface for handling byzantine behaviour. Implementations of this should watch for
/// eclipse attacks, as well as invalid state transitions.
#[async_trait::async_trait]
pub trait ByzantineHandler {
	/// Returns the [`ConsensusMessage`] that caused the emission of  [`ChallengePeriodStarted`]
	/// event
	async fn query_consensus_message(
		&self,
		challenge_event: ChallengePeriodStarted,
	) -> Result<ConsensusMessage, anyhow::Error>;

	/// Check the client message for byzantine behaviour and submit it to the chain if any.
	async fn check_for_byzantine_attack<C: IsmpHost>(
		&self,
		counterparty: &C,
		consensus_message: ConsensusMessage,
	) -> Result<(), anyhow::Error>;
}

/// Provides an interface for the chain to the relayer core for submitting Ismp messages as well as
#[async_trait::async_trait]
pub trait IsmpHost: ByzantineHandler + Reconnect + Clone + Send + Sync {
	/// Return a stream that yields [`ConsensusMessage`] when a new consensus update
	/// can be sent to the counterparty
	async fn consensus_notification<C>(
		&self,
		counterparty: C,
	) -> Result<BoxStream<ConsensusMessage>, anyhow::Error>
	where
		C: IsmpHost + IsmpProvider + Clone + 'static;
}

#[async_trait::async_trait]
pub trait Reconnect: Clone + Send + Sync {
	/// Recreate all underline network connections
	async fn reconnect<C: IsmpProvider>(&mut self, counterparty: &C) -> Result<(), anyhow::Error>;
}

#[derive(Clone, Debug)]
pub struct NonceProvider {
	nonce: Arc<tokio::sync::Mutex<u64>>,
}

impl NonceProvider {
	pub fn new(nonce: u64) -> Self {
		Self { nonce: Arc::new(tokio::sync::Mutex::new(nonce)) }
	}

	pub async fn get_nonce(&self) -> u64 {
		let mut guard = self.nonce.lock().await;
		let nonce = *guard;
		*guard = nonce + 1;
		nonce
	}
}

pub async fn reconnect_with_exponential_back_off<A: IsmpProvider, B: IsmpProvider>(
	chain: &mut A,
	counterparty: &B,
	reconnects: u32,
) -> Result<(), anyhow::Error> {
	let mut initial_backoff = 1;
	for _ in 0..reconnects {
		// If backoff is more than 512 seconds reset backoff
		if let Ok(()) = chain.reconnect(counterparty).await {
			return Ok(())
		}
		if initial_backoff == 512 {
			initial_backoff = 1;
		}
		initial_backoff = initial_backoff * 2;
		tokio::time::sleep(Duration::from_secs(initial_backoff)).await;
	}
	return Err(anyhow!("Failed to reconnect after {} tries", reconnects))
}
