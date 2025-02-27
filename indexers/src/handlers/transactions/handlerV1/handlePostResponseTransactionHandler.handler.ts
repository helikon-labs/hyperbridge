import { HyperBridgeService } from "../../../services/hyperbridge.service";
import { RelayerService } from "../../../services/relayer.service";
import { HandlePostRequestsTransaction } from "../../../types/abi-interfaces/HandlerV1Abi";
import StateMachineHelpers from "../../../utils/stateMachine.helpers";

/**
 * Handles the handlePostResponse transaction from handlerV1 contract
 */
export async function handlePostResponseTransactionHandler(
  transaction: HandlePostRequestsTransaction
): Promise<void> {
  const { blockNumber, hash } = transaction;

  logger.info(
    `Handling PostRequests Transaction: ${JSON.stringify({
      blockNumber,
      transactionHash: hash,
    })}`
  );

  const chain: string =
    StateMachineHelpers.getEvmStateMachineIdFromTransaction(transaction);

  Promise.all([
    await RelayerService.handlePostRequestOrResponseTransaction(
      chain,
      transaction
    ),
    await HyperBridgeService.handlePostRequestOrResponseTransaction(
      chain,
      transaction
    ),
  ]);
}
