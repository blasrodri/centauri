// Copyright 2022 ComposableFi
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![warn(unused_variables)]

use futures::{future::ready, StreamExt};
use primitives::Chain;

pub mod chain;
pub mod command;
pub mod events;
pub mod logging;
mod macros;
pub mod packets;
pub mod queue;

use events::{has_packet_events, parse_events};
use ibc::events::IbcEvent;
use metrics::handler::MetricsHandler;

/// Core relayer loop, waits for new finality events and forwards any new [`ibc::IbcEvents`]
/// to the counter party chain.
pub async fn relay<A, B>(
	mut chain_a: A,
	mut chain_b: B,
	mut chain_a_metrics: Option<MetricsHandler>,
	mut chain_b_metrics: Option<MetricsHandler>,
) -> Result<(), anyhow::Error>
where
	A: Chain,
	B: Chain,
{
	let (mut chain_a_finality, mut chain_b_finality) =
		(chain_a.finality_notifications().await, chain_b.finality_notifications().await);
	// loop forever
	loop {
		tokio::select! {
			// new finality event from chain A
			result = chain_a_finality.next() => {
				process_finality_event!(chain_a, chain_b, chain_a_metrics, result)
			}
			// new finality event from chain B
			result = chain_b_finality.next() => {
				process_finality_event!(chain_b, chain_a, chain_b_metrics, result)
			}
		}
	}

	Ok(())
}

pub async fn fish<A, B>(chain_a: A, chain_b: B) -> Result<(), anyhow::Error>
where
	A: Chain,
	A::Error: From<B::Error>,
	B: Chain,
	B::Error: From<A::Error>,
{
	// we only care about events where the counterparty light client is updated.
	let (mut chain_a_client_updates, mut chain_b_client_updates) = (
		chain_a.ibc_events().await.filter_map(|ev| {
			ready(match ev {
				IbcEvent::UpdateClient(update) if chain_b.client_id() == *update.client_id() =>
					Some(update),
				_ => None,
			})
		}),
		chain_b.ibc_events().await.filter_map(|ev| {
			ready(match ev {
				IbcEvent::UpdateClient(update) if chain_a.client_id() == *update.client_id() =>
					Some(update),
				_ => None,
			})
		}),
	);

	// loop forever
	loop {
		tokio::select! {
			// new finality event from chain A
			update = chain_a_client_updates.next() => {
				let update = match update {
					Some(update) => update,
					None => break,
				};
				let message = chain_a.query_client_message(update).await?;
				chain_b.check_for_misbehaviour(&chain_a, message).await?;
			}
			// new finality event from chain B
			update = chain_b_client_updates.next() => {
				let update = match update {
					Some(update) => update,
					None => break,
				};
				let message = chain_b.query_client_message(update).await?;
				chain_a.check_for_misbehaviour(&chain_b, message).await?;
			}
		}
	}

	Ok(())
}

#[cfg(feature = "testing")]
pub mod send_packet_relay {
	use std::sync::atomic::{AtomicBool, Ordering};
	static RELAY_PACKETS: AtomicBool = AtomicBool::new(true);

	/// Returns status of send packet relay
	pub fn packet_relay_status() -> bool {
		RELAY_PACKETS.load(Ordering::SeqCst)
	}

	/// Sets packet relay status
	pub fn set_relay_status(status: bool) {
		RELAY_PACKETS.store(status, Ordering::SeqCst);
	}
}
