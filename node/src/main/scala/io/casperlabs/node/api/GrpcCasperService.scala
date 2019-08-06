package io.casperlabs.node.api

import cats.effect._
import cats.implicits._
import com.google.protobuf.empty.Empty
import com.google.protobuf.ByteString
import io.casperlabs.blockstorage.BlockStore
import io.casperlabs.casper.MultiParentCasperRef.MultiParentCasperRef
import io.casperlabs.casper.SafetyOracle
import io.casperlabs.casper.api.BlockAPI
import io.casperlabs.casper.consensus.info._
import io.casperlabs.catscontrib.MonadThrowable
import io.casperlabs.metrics.Metrics
import io.casperlabs.node.api.casper._
import io.casperlabs.shared.Log
import io.casperlabs.comm.ServiceError.InvalidArgument
import io.casperlabs.smartcontracts.ExecutionEngineService
import io.casperlabs.models.SmartContractEngineError
import io.casperlabs.ipc
import monix.execution.Scheduler
import monix.eval.{Task, TaskLike}
import monix.reactive.Observable

object GrpcCasperService extends StateConversions {

  def apply[F[_]: Concurrent: TaskLike: Log: Metrics: MultiParentCasperRef: SafetyOracle: BlockStore: ExecutionEngineService](
      ignoreDeploySignature: Boolean
  ): F[CasperGrpcMonix.CasperService] =
    BlockAPI.establishMetrics[F] *> Sync[F].delay {
      new CasperGrpcMonix.CasperService {
        override def deploy(request: DeployRequest): Task[Empty] =
          TaskLike[F].toTask {
            BlockAPI.deploy[F](request.getDeploy, ignoreDeploySignature).map(_ => Empty())
          }

        override def getBlockInfo(request: GetBlockInfoRequest): Task[BlockInfo] =
          TaskLike[F].toTask {
            BlockAPI
              .getBlockInfo[F](
                request.blockHashBase16,
                full = request.view == BlockInfoView.FULL
              )
          }

        override def streamBlockInfos(request: StreamBlockInfosRequest): Observable[BlockInfo] = {
          val infos = TaskLike[F].toTask {
            BlockAPI.getBlockInfos[F](
              depth = request.depth,
              maxRank = request.maxRank,
              full = request.view == BlockInfoView.FULL
            )
          }
          Observable.fromTask(infos).flatMap(Observable.fromIterable)
        }

        def getBlockState(request: GetBlockStateRequest): Task[State.Value] =
          batchGetBlockState(
            BatchGetBlockStateRequest(request.blockHashBase16, List(request.getQuery))
          ) map {
            _.values.head
          }

        def batchGetBlockState(
            request: BatchGetBlockStateRequest
        ): Task[BatchGetBlockStateResponse] = TaskLike[F].toTask {
          for {
            info      <- BlockAPI.getBlockInfo[F](request.blockHashBase16)
            stateHash = info.getSummary.getHeader.getState.postStateHash
            values    <- request.queries.toList.traverse(getState(stateHash, _))
          } yield BatchGetBlockStateResponse(values)
        }

        private def getState(stateHash: ByteString, query: StateQuery): F[State.Value] =
          for {
            key <- toKey[F](query.keyVariant, query.keyBase16)
            possibleResponse <- ExecutionEngineService[F].query(
                                 stateHash,
                                 key,
                                 query.pathSegments
                               )
            value <- Concurrent[F].fromEither(possibleResponse).handleErrorWith {
                      case SmartContractEngineError(msg) =>
                        MonadThrowable[F].raiseError(InvalidArgument(msg))
                    }
          } yield fromIpc(value)
      }
    }

  def toKey[F[_]: MonadThrowable](keyType: StateQuery.KeyVariant, keyValue: String): F[ipc.Key] =
    GrpcDeployService.toKey[F](keyType.name, keyValue).handleErrorWith {
      case ex: java.lang.IllegalArgumentException =>
        MonadThrowable[F].raiseError(InvalidArgument(ex.getMessage))
    }
}
