use crate::{
	abi::{
		GetRequest, GetResponseMessage, GetTimeoutMessage, Handler as IsmpHandler, PostRequestLeaf,
		PostRequestMessage, PostResponseLeaf, PostResponseMessage, Proof,
	},
	EvmClient,
};
use anyhow::{anyhow, Error};
use codec::Decode;
use ethers::{prelude::Ws, providers::PendingTransaction};
use ismp::{
	host::StateMachine,
	messaging::{Message, ResponseMessage, TimeoutMessage},
	router::{Request, RequestResponse, Response},
};
use ismp_rpc::MmrProof;
use ismp_solidity_abi::{
	handler::{PostRequestTimeoutMessage, PostResponseTimeoutMessage},
	shared_types::StateMachineHeight,
};
use merkle_mountain_range::mmr_position_to_k_index;
use pallet_ismp::{primitives::SubstrateStateProof, NodesUtils};
use sp_core::H256;
use tesseract_primitives::IsmpHost;

/// Use this to initialize the transaction submit queue. This pipelines transaction submission
/// eliminating race conditions.
pub async fn submit_messages<I: IsmpHost>(
	client: &EvmClient<I>,
	messages: Vec<Message>,
) -> anyhow::Result<()> {
	let contract = IsmpHandler::new(client.handler, client.signer.clone());
	let ismp_host = client.ismp_host;
	for msg in messages {
		let nonce = client.get_nonce().await?;
		match msg {
			Message::Consensus(msg) => {
				match contract
					.handle_consensus(ismp_host, msg.consensus_proof.into())
					.nonce(nonce)
					.gas(client.gas_limit)
					.send()
					.await
				{
					Ok(progress) => wait_for_success(progress, Some(2)).await,
					Err(err) => {
						log::error!("Error broadcasting transaction for  {err:?}");
					},
				}
			},
			Message::Request(msg) => {
				let membership_proof =
					match MmrProof::<H256>::decode(&mut msg.proof.proof.as_slice()) {
						Ok(proof) => proof,
						_ => {
							log::error!("Failed to decode membership proof");
							continue
						},
					};
				let mmr_size = NodesUtils::new(membership_proof.leaf_count).size();
				let k_and_leaf_indices = membership_proof
					.leaf_positions_and_indices
					.into_iter()
					.map(|(pos, leaf_index)| {
						let k_index = mmr_position_to_k_index(vec![pos], mmr_size)[0].1;
						(k_index, leaf_index)
					})
					.collect::<Vec<_>>();

				let mut leaves = msg
					.requests
					.into_iter()
					.zip(k_and_leaf_indices)
					.map(|(post, (k_index, leaf_index))| PostRequestLeaf {
						request: post.into(),
						index: leaf_index.into(),
						k_index: k_index.into(),
					})
					.collect::<Vec<_>>();
				leaves.sort_by_key(|leaf| leaf.index);

				let post_message = PostRequestMessage {
					proof: Proof {
						height: StateMachineHeight {
							state_machine_id: {
								match msg.proof.height.id.state_id {
									StateMachine::Polkadot(id) | StateMachine::Kusama(id) =>
										id.into(),
									_ => {
										panic!("Expected polkadot or kusama state machines");
									},
								}
							},
							height: msg.proof.height.height.into(),
						},
						multiproof: membership_proof.items.into_iter().map(|node| node.0).collect(),
						leaf_count: membership_proof.leaf_count.into(),
					},
					requests: leaves,
				};

				match contract
					.handle_post_requests(ismp_host, post_message)
					.nonce(nonce)
					.gas(client.gas_limit)
					.send()
					.await
				{
					Ok(progress) => wait_for_success(progress, None).await,
					Err(err) => {
						log::error!("Error broadcasting transaction for  {err:?}");
					},
				}
			},
			Message::Response(ResponseMessage { datagram, proof, .. }) => {
				let membership_proof = match MmrProof::<H256>::decode(&mut proof.proof.as_slice()) {
					Ok(proof) => proof,
					_ => {
						log::error!("Failed to decode membership proof");
						continue
					},
				};
				let mmr_size = NodesUtils::new(membership_proof.leaf_count).size();
				let k_and_leaf_indices = membership_proof
					.leaf_positions_and_indices
					.into_iter()
					.map(|(pos, leaf_index)| {
						let k_index = mmr_position_to_k_index(vec![pos], mmr_size)[0].1;
						(k_index, leaf_index)
					})
					.collect::<Vec<_>>();

				match datagram {
					RequestResponse::Response(responses) => {
						let mut leaves = responses
							.into_iter()
							.zip(k_and_leaf_indices)
							.filter_map(|(res, (k_index, leaf_index))| match res {
								Response::Post(res) => Some(PostResponseLeaf {
									response: res.into(),
									index: leaf_index.into(),
									k_index: k_index.into(),
								}),
								_ => None,
							})
							.collect::<Vec<_>>();
						leaves.sort_by_key(|leaf| leaf.index);

						let message =
							PostResponseMessage {
								proof: Proof {
									height: StateMachineHeight {
										state_machine_id: {
											match proof.height.id.state_id {
												StateMachine::Polkadot(id) |
												StateMachine::Kusama(id) => id.into(),
												_ => {
													log::error!("Expected polkadot or kusama state machines");
													continue
												},
											}
										},
										height: proof.height.height.into(),
									},
									multiproof: membership_proof
										.items
										.into_iter()
										.map(|node| node.0)
										.collect(),
									leaf_count: membership_proof.leaf_count.into(),
								},
								responses: leaves,
							};
						match contract
							.handle_post_responses(ismp_host, message)
							.nonce(nonce)
							.gas(client.gas_limit)
							.send()
							.await
						{
							Ok(progress) => wait_for_success(progress, None).await,
							Err(err) => {
								log::error!("Error broadcasting transaction for  {err:?}");
							},
						}
					},
					RequestResponse::Request(requests) => {
						let requests = match requests
							.into_iter()
							.map(|req| {
								let get = req
									.get_request()
									.map_err(|_| anyhow!("Expected get request"))?;
								Ok(GetRequest {
									source: get.source.to_string().as_bytes().to_vec().into(),
									dest: get.dest.to_string().as_bytes().to_vec().into(),
									nonce: get.nonce,
									from: get.from.into(),
									keys: get.keys.into_iter().map(|key| key.into()).collect(),
									timeout_timestamp: get.timeout_timestamp,
									gaslimit: get.gas_limit.into(),
									height: get.height.into(),
								})
							})
							.collect::<Result<Vec<_>, Error>>()
						{
							Ok(reqs) => reqs,
							Err(err) => {
								log::error!("Failed to error {err:?}");
								continue
							},
						};

						let state_proof: SubstrateStateProof =
							match codec::Decode::decode(&mut proof.proof.as_slice()) {
								Ok(proof) => proof,
								_ => {
									log::error!("Failed to decode membership proof");
									continue
								},
							};
						let message = GetResponseMessage {
							proof: state_proof
								.storage_proof
								.into_iter()
								.map(|key| key.into())
								.collect(),
							height: StateMachineHeight {
								state_machine_id: {
									match proof.height.id.state_id {
										StateMachine::Polkadot(id) | StateMachine::Kusama(id) =>
											id.into(),
										_ => {
											log::error!(
												"Expected polkadot or kusama state machines"
											);
											continue
										},
									}
								},
								height: proof.height.height.into(),
							},
							requests,
						};

						match contract
							.handle_get_responses(ismp_host, message)
							.nonce(nonce)
							.gas(client.gas_limit)
							.send()
							.await
						{
							Ok(progress) => wait_for_success(progress, None).await,
							Err(err) => {
								log::error!("Error broadcasting transaction for  {err:?}");
							},
						}
					},
				}
			},
			Message::Timeout(TimeoutMessage::Post { timeout_proof, requests }) => {
				let post_requests = requests
					.into_iter()
					.filter_map(|req| match req {
						Request::Post(post) => Some(post.into()),
						Request::Get(_) => None,
					})
					.collect();

				let state_proof: SubstrateStateProof =
					match codec::Decode::decode(&mut timeout_proof.proof.as_slice()) {
						Ok(proof) => proof,
						_ => {
							log::error!("Failed to decode membership proof");
							continue
						},
					};
				let message = PostRequestTimeoutMessage {
					timeouts: post_requests,
					height: StateMachineHeight {
						state_machine_id: {
							match timeout_proof.height.id.state_id {
								StateMachine::Polkadot(id) | StateMachine::Kusama(id) => id.into(),
								_ => {
									log::error!("Expected polkadot or kusama state machines");
									continue
								},
							}
						},
						height: timeout_proof.height.height.into(),
					},
					proof: state_proof.storage_proof.into_iter().map(|key| key.into()).collect(),
				};

				match contract
					.handle_post_request_timeouts(ismp_host, message)
					.nonce(nonce)
					.gas(client.gas_limit)
					.send()
					.await
				{
					Ok(progress) => wait_for_success(progress, None).await,
					Err(err) => {
						log::error!("Error broadcasting transaction for  {err:?}");
					},
				}
			},

			Message::Timeout(TimeoutMessage::PostResponse { timeout_proof, responses }) => {
				let post_responses = responses.into_iter().map(|res| res.into()).collect();

				let state_proof: SubstrateStateProof =
					match codec::Decode::decode(&mut timeout_proof.proof.as_slice()) {
						Ok(proof) => proof,
						_ => {
							log::error!("Failed to decode membership proof");
							continue
						},
					};
				let message = PostResponseTimeoutMessage {
					timeouts: post_responses,
					height: StateMachineHeight {
						state_machine_id: {
							match timeout_proof.height.id.state_id {
								StateMachine::Polkadot(id) | StateMachine::Kusama(id) => id.into(),
								_ => {
									log::error!("Expected polkadot or kusama state machines");
									continue
								},
							}
						},
						height: timeout_proof.height.height.into(),
					},
					proof: state_proof.storage_proof.into_iter().map(|key| key.into()).collect(),
				};

				match contract
					.handle_post_response_timeouts(ismp_host, message)
					.nonce(nonce)
					.gas(client.gas_limit)
					.send()
					.await
				{
					Ok(progress) => wait_for_success(progress, None).await,
					Err(err) => {
						log::error!("Error broadcasting transaction for  {err:?}");
					},
				}
			},
			Message::Timeout(TimeoutMessage::Get { requests }) => {
				let get_requests = requests
					.into_iter()
					.filter_map(|req| match req {
						Request::Get(get) => Some(GetRequest {
							source: get.source.to_string().as_bytes().to_vec().into(),
							dest: get.dest.to_string().as_bytes().to_vec().into(),
							nonce: get.nonce,
							from: get.from.into(),
							keys: get.keys.into_iter().map(|key| key.into()).collect(),
							timeout_timestamp: get.timeout_timestamp,
							gaslimit: get.gas_limit.into(),
							height: get.height.into(),
						}),
						_ => None,
					})
					.collect();

				let message = GetTimeoutMessage { timeouts: get_requests };

				match contract
					.handle_get_request_timeouts(ismp_host, message)
					.nonce(nonce)
					.gas(client.gas_limit)
					.send()
					.await
				{
					Ok(progress) => wait_for_success(progress, None).await,
					Err(err) => {
						log::error!("Error broadcasting transaction for  {err:?}");
					},
				}
			},
			_ => {
				log::debug!(target: "tesseract", "Message handler not implemented in solidity abi")
			},
		}
	}

	Ok(())
}

async fn wait_for_success<'a>(tx: PendingTransaction<'a, Ws>, confirmations: Option<usize>) {
	if let Err(err) = tx.confirmations(confirmations.unwrap_or(1)).await {
		log::error!("Error broadcasting transaction for  {err:?}");
	}
}
