use anyhow::{anyhow, Error};
use codec::Encode;
use consensus_client::types::ConsensusState;
use futures::stream;
use ismp::{
	consensus::{ConsensusStateId, StateMachineId},
	events::Event,
	messaging::{CreateConsensusState, Message},
	router::Get,
};
use primitive_types::H256;
use std::{
	sync::{Arc, Mutex},
	time::Duration,
};
use tesseract_primitives::{
	BoxStream, ByzantineHandler, IsmpHost, IsmpProvider, NonceProvider, Query, Reconnect,
	Signature, StateMachineUpdated,
};

#[derive(Clone)]
pub struct MockHost {
	pub consensus_state: Arc<Mutex<ConsensusState>>,
	pub latest_height: Arc<Mutex<u64>>,
}

impl MockHost {
	pub fn new(consensus_state: ConsensusState, latest_height: u64) -> Self {
		Self {
			consensus_state: Arc::new(Mutex::new(consensus_state)),
			latest_height: Arc::new(Mutex::new(latest_height)),
		}
	}
}

#[async_trait::async_trait]
impl ByzantineHandler for MockHost {
	async fn query_consensus_message(
		&self,
		_challenge_event: StateMachineUpdated,
	) -> Result<ismp::messaging::ConsensusMessage, anyhow::Error> {
		Err(anyhow!("No consensus messages"))
	}

	async fn check_for_byzantine_attack<T: IsmpHost>(
		&self,
		_counterparty: &T,
		_consensus_message: ismp::messaging::ConsensusMessage,
	) -> Result<(), anyhow::Error> {
		Err(anyhow!("No byzantine faults"))
	}
}

#[async_trait::async_trait]
impl IsmpHost for MockHost {
	async fn consensus_notification<I>(
		&self,
		_counterparty: I,
	) -> Result<BoxStream<ismp::messaging::ConsensusMessage>, anyhow::Error>
	where
		I: IsmpHost + IsmpProvider + Clone + 'static,
	{
		Ok(Box::pin(stream::pending()))
	}

	async fn get_initial_consensus_state(&self) -> Result<Option<CreateConsensusState>, Error> {
		todo!()
	}
}

#[async_trait::async_trait]
impl IsmpProvider for MockHost {
	async fn query_consensus_state(
		&self,
		_at: Option<u64>,
		_id: ConsensusStateId,
	) -> Result<Vec<u8>, anyhow::Error> {
		Ok(self.consensus_state.lock().unwrap().encode())
	}

	async fn query_latest_height(&self, _id: StateMachineId) -> Result<u32, anyhow::Error> {
		Ok(*self.latest_height.lock().unwrap() as u32)
	}

	async fn query_latest_messaging_height(
		&self,
		_id: StateMachineId,
	) -> Result<u64, anyhow::Error> {
		Ok(*self.latest_height.lock().unwrap() as u64)
	}

	async fn query_consensus_update_time(
		&self,
		_id: ConsensusStateId,
	) -> Result<Duration, anyhow::Error> {
		Ok(Duration::from_secs(0))
	}

	async fn query_challenge_period(
		&self,
		_id: ConsensusStateId,
	) -> Result<Duration, anyhow::Error> {
		Ok(Duration::from_secs(0))
	}

	async fn query_timestamp(&self) -> Result<Duration, anyhow::Error> {
		Ok(Duration::from_secs(0))
	}

	async fn query_requests_proof(
		&self,
		_at: u64,
		_keys: Vec<Query>,
	) -> Result<Vec<u8>, anyhow::Error> {
		Ok(Default::default())
	}

	async fn query_responses_proof(
		&self,
		_at: u64,
		_keys: Vec<Query>,
	) -> Result<Vec<u8>, anyhow::Error> {
		Ok(Default::default())
	}

	async fn query_state_proof(
		&self,
		_at: u64,
		_keys: Vec<Vec<u8>>,
	) -> Result<Vec<u8>, anyhow::Error> {
		Ok(Default::default())
	}

	async fn query_ismp_events(
		&self,
		_previous_height: u64,
		_event: StateMachineUpdated,
	) -> Result<Vec<Event>, anyhow::Error> {
		todo!()
	}

	fn name(&self) -> String {
		"Mock".to_string()
	}

	fn state_machine_id(&self) -> StateMachineId {
		todo!()
	}

	fn block_max_gas(&self) -> u64 {
		todo!()
	}

	fn initial_height(&self) -> u64 {
		0
	}

	async fn estimate_gas(&self, _msg: Vec<Message>) -> Result<u64, anyhow::Error> {
		todo!()
	}

	async fn state_machine_update_notification(
		&self,
		_counterparty_state_id: StateMachineId,
	) -> Result<BoxStream<StateMachineUpdated>, anyhow::Error> {
		todo!()
	}

	async fn submit(&self, _messages: Vec<Message>) -> Result<(), anyhow::Error> {
		todo!()
	}

	fn request_commitment_full_key(&self, commitment: H256) -> Vec<u8> {
		Default::default()
	}

	fn request_receipt_full_key(&self, commitment: H256) -> Vec<u8> {
		Default::default()
	}

	fn response_commitment_full_key(&self, commitment: H256) -> Vec<u8> {
		Default::default()
	}

	fn response_receipt_full_key(&self, commitment: H256) -> Vec<u8> {
		Default::default()
	}

	fn address(&self) -> Vec<u8> {
		Default::default()
	}

	fn sign(&self, msg: &[u8]) -> Signature {
		todo!()
	}

	async fn initialize_nonce(&self) -> Result<NonceProvider, Error> {
		todo!()
	}

	fn set_nonce_provider(&mut self, nonce_provider: NonceProvider) {
		todo!()
	}

	async fn set_initial_consensus_state(
		&self,
		message: CreateConsensusState,
	) -> Result<(), Error> {
		todo!()
	}
}

#[async_trait::async_trait]
impl Reconnect for MockHost {
	async fn reconnect<C: IsmpProvider>(&mut self, _counterparty: &C) -> Result<(), anyhow::Error> {
		Ok(())
	}
}
