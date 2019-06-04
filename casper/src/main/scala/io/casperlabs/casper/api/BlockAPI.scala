package io.casperlabs.casper.api

import cats.Monad
import cats.effect.concurrent.Semaphore
import cats.effect.{Concurrent, Sync}
import cats.implicits._
import com.google.protobuf.ByteString
import io.casperlabs.blockstorage.{BlockDagRepresentation, BlockStore, StorageError}
import io.casperlabs.casper.Estimator.BlockHash
import io.casperlabs.casper.MultiParentCasperRef.MultiParentCasperRef
import io.casperlabs.casper.consensus._
import io.casperlabs.casper.consensus.info._
import io.casperlabs.casper.protocol.{
  BlockInfoWithoutTuplespace,
  BlockQuery,
  BlockQueryResponse,
  DeployServiceResponse,
  BlockInfo => BlockInfoWithTuplespace
}
import io.casperlabs.casper.util.ProtoUtil
import io.casperlabs.casper.{protocol, BlockStatus => _, _}
import io.casperlabs.catscontrib.MonadThrowable
import io.casperlabs.comm.ServiceError
import io.casperlabs.comm.ServiceError._
import io.casperlabs.crypto.codec.Base16
import io.casperlabs.metrics.Metrics
import io.casperlabs.shared.Log

object BlockAPI {

  private implicit val metricsSource: Metrics.Source =
    Metrics.Source(CasperMetricsSource, "block-api")

  private def unsafeWithCasper[F[_]: MonadThrowable: Log: MultiParentCasperRef, A](
      msg: String
  )(f: MultiParentCasper[F] => F[A]): F[A] =
    MultiParentCasperRef
      .withCasper[F, A](
        f,
        msg,
        MonadThrowable[F].raiseError(Unavailable("Casper instance not available yet."))
      )

  /** Export base 0 values so we have non-empty series for charts. */
  def establishMetrics[F[_]: Monad: Metrics] =
    for {
      _ <- Metrics[F].incrementCounter("deploys", 0)
      _ <- Metrics[F].incrementCounter("deploys-success", 0)
      _ <- Metrics[F].incrementCounter("create-blocks", 0)
      _ <- Metrics[F].incrementCounter("create-blocks-success", 0)
    } yield ()

  @deprecated("To be removed before devnet. Use the one with `Deploy`.", "0.4")
  def deploy[F[_]: MonadThrowable: MultiParentCasperRef: Log: Metrics](
      d: protocol.DeployData,
      ignoreDeploySignature: Boolean
  ): F[DeployServiceResponse] = {
    def casperDeploy(implicit casper: MultiParentCasper[F]): F[DeployServiceResponse] =
      for {
        _ <- Metrics[F].incrementCounter("deploys")
        _ <- MonadThrowable[F]
              .raiseError {
                Unimplemented(
                  "Signature check on protocol.DeployData is not implemented. Use CasperService."
                )
              }
              .whenA(!ignoreDeploySignature)
        n = LegacyConversions.toDeploy(d)
        r <- MultiParentCasper[F].deploy(n)
        re <- r match {
               case Right(_) =>
                 Metrics[F].incrementCounter("deploys-success") *>
                   DeployServiceResponse(success = true, "Success!").pure[F]
               case Left(err) =>
                 DeployServiceResponse(success = false, err.getMessage).pure[F]
             }
      } yield re

    val errorMessage = "Could not deploy."

    MultiParentCasperRef
      .withCasper[F, DeployServiceResponse](
        casperDeploy(_),
        errorMessage,
        DeployServiceResponse(success = false, s"Error: $errorMessage").pure[F]
      )
  }

  def deploy[F[_]: MonadThrowable: MultiParentCasperRef: Log: Metrics](
      d: Deploy,
      ignoreDeploySignature: Boolean
  ): F[Unit] = unsafeWithCasper[F, Unit]("Could not deploy.") { implicit casper =>
    def check(msg: String)(f: F[Boolean]): F[Unit] =
      f flatMap { ok =>
        MonadThrowable[F].raiseError(InvalidArgument(msg)).whenA(!ok)
      }

    for {
      _ <- Metrics[F].incrementCounter("deploys")
      // Doing these here while MultiParentCasper is still using the legacy deploys.
      _ <- check("Invalid deploy hash.")(Validate.deployHash[F](d))
      _ <- check("Invalid deploy signature.")(Validate.deploySignature[F](d))
            .whenA(!ignoreDeploySignature)
      r <- MultiParentCasper[F].deploy(d)
      _ <- r match {
            case Right(_) =>
              Metrics[F].incrementCounter("deploys-success") *> ().pure[F]
            case Left(ex: IllegalArgumentException) =>
              MonadThrowable[F].raiseError[Unit](InvalidArgument(ex.getMessage))
            case Left(ex) =>
              MonadThrowable[F].raiseError[Unit](ex)
          }
    } yield ()
  }

  @deprecated("To be removed before devnet. Use `propose`.", "0.4")
  def createBlock[F[_]: Concurrent: MultiParentCasperRef: Log: Metrics](
      blockApiLock: Semaphore[F]
  ): F[DeployServiceResponse] =
    propose(blockApiLock) map { blockHash =>
      val hash = PrettyPrinter.buildString(blockHash)
      DeployServiceResponse(success = true, s"Success! Block $hash created and added.")
    } handleError {
      case InvalidArgument(msg) =>
        DeployServiceResponse(success = false, s"Failure! $msg")
      case Internal(msg) =>
        DeployServiceResponse(success = false, msg)
      case Aborted(msg) =>
        DeployServiceResponse(success = false, s"Error: $msg")
      case FailedPrecondition(msg) =>
        DeployServiceResponse(success = false, s"Error while creating block: $msg")
      case OutOfRange(msg) =>
        DeployServiceResponse(success = false, s"Error while creating block: $msg")
      case Unavailable(_) =>
        DeployServiceResponse(success = false, s"Error: Could not create block.")
    }

  def propose[F[_]: Sync: MultiParentCasperRef: Log: Metrics](
      blockApiLock: Semaphore[F]
  ): F[ByteString] = {
    def raise[A](ex: ServiceError.Exception): F[ByteString] =
      MonadThrowable[F].raiseError(ex)

    unsafeWithCasper[F, ByteString]("Could not create block.") { implicit casper =>
      Sync[F].bracket[Boolean, ByteString](blockApiLock.tryAcquire) {
        case true =>
          for {
            _          <- Metrics[F].incrementCounter("create-blocks")
            maybeBlock <- casper.createBlock
            result <- maybeBlock match {
                       case Created(block) =>
                         for {
                           status <- casper.addBlock(block)
                           res <- status match {
                                   case _: ValidBlock =>
                                     block.blockHash.pure[F]
                                   case _: InvalidBlock =>
                                     raise(InvalidArgument(s"Invalid block: $status"))
                                   case BlockException(ex) =>
                                     raise(Internal(s"Error during block processing: $ex"))
                                   case Processing | Processed =>
                                     raise(
                                       Aborted(
                                         "No action taken since other thread is already processing the block."
                                       )
                                     )
                                 }
                           _ <- Metrics[F].incrementCounter("create-blocks-success")
                         } yield res

                       case InternalDeployError(ex) =>
                         raise(Internal(ex.getMessage))

                       case ReadOnlyMode =>
                         raise(FailedPrecondition("Node is in read-only mode."))

                       case NoNewDeploys =>
                         raise(OutOfRange("No new deploys."))
                     }
          } yield result

        case false =>
          raise(Aborted("There is another propose in progress."))
      }(blockApiLock.release.whenA(_))
    }
  }

  // FIX: Not used at the moment - in RChain it's being used in method like `getListeningName*`
  @deprecated("To be removed before devnet.", "0.4")
  private def getMainChainFromTip[F[_]: MonadThrowable: MultiParentCasper: Log: SafetyOracle: BlockStore](
      depth: Int
  ): F[IndexedSeq[Block]] =
    for {
      dag       <- MultiParentCasper[F].blockDag
      tipHashes <- MultiParentCasper[F].estimator(dag)
      tipHash   = tipHashes.head
      tip       <- ProtoUtil.unsafeGetBlock[F](tipHash)
      mainChain <- ProtoUtil.getMainChainUntilDepth[F](tip, IndexedSeq.empty[Block], depth)
    } yield mainChain

  // TOOD extract common code from show blocks
  @deprecated("To be removed before devnet. Use `getBlockInfos`.", "0.4")
  def showBlocks[F[_]: MonadThrowable: MultiParentCasperRef: Log: SafetyOracle: BlockStore](
      depth: Int
  ): F[List[BlockInfoWithoutTuplespace]] = {
    val errorMessage =
      "Could not show blocks."

    def casperResponse(implicit casper: MultiParentCasper[F]) =
      for {
        dag <- MultiParentCasper[F].blockDag
        flattenedBlockInfosUntilDepth <- getFlattenedBlockInfosUntilDepth[F](
                                          depth,
                                          dag
                                        )
      } yield flattenedBlockInfosUntilDepth.reverse

    MultiParentCasperRef.withCasper[F, List[BlockInfoWithoutTuplespace]](
      casperResponse(_),
      errorMessage,
      List.empty[BlockInfoWithoutTuplespace].pure[F]
    )
  }

  private def getFlattenedBlockInfosUntilDepth[F[_]: MonadThrowable: MultiParentCasper: Log: SafetyOracle: BlockStore](
      depth: Int,
      dag: BlockDagRepresentation[F]
  ): F[List[BlockInfoWithoutTuplespace]] =
    for {
      topoSort <- dag.topoSortTail(depth)
      result <- topoSort.foldM(List.empty[BlockInfoWithoutTuplespace]) {
                 case (blockInfosAtHeightAcc, blockHashesAtHeight) =>
                   for {
                     blocksAtHeight     <- blockHashesAtHeight.traverse(ProtoUtil.unsafeGetBlock[F])
                     blockInfosAtHeight <- blocksAtHeight.traverse(getBlockInfoWithoutTuplespace[F])
                   } yield blockInfosAtHeightAcc ++ blockInfosAtHeight
               }
    } yield result

  @deprecated("To be removed before devnet.", "0.4")
  def showMainChain[F[_]: MonadThrowable: MultiParentCasperRef: Log: SafetyOracle: BlockStore](
      depth: Int
  ): F[List[BlockInfoWithoutTuplespace]] = {
    val errorMessage =
      "Could not show main chain."

    def casperResponse(implicit casper: MultiParentCasper[F]) =
      for {
        dag        <- MultiParentCasper[F].blockDag
        tipHashes  <- MultiParentCasper[F].estimator(dag)
        tipHash    = tipHashes.head
        tip        <- ProtoUtil.unsafeGetBlock[F](tipHash)
        mainChain  <- ProtoUtil.getMainChainUntilDepth[F](tip, IndexedSeq.empty[Block], depth)
        blockInfos <- mainChain.toList.traverse(getBlockInfoWithoutTuplespace[F])
      } yield blockInfos

    MultiParentCasperRef.withCasper[F, List[BlockInfoWithoutTuplespace]](
      casperResponse(_),
      errorMessage,
      List.empty[BlockInfoWithoutTuplespace].pure[F]
    )
  }

  // TODO: Replace with call to BlockStore
  @deprecated("To be removed before devnet. Will add `getDeployInfo`.", "0.4")
  def findBlockWithDeploy[F[_]: MonadThrowable: MultiParentCasperRef: Log: SafetyOracle: BlockStore](
      accountPublicKey: ByteString,
      timestamp: Long
  ): F[BlockQueryResponse] = {
    val errorMessage =
      "Could not find block with deploy."

    def casperResponse(implicit casper: MultiParentCasper[F]): F[BlockQueryResponse] =
      for {
        dag               <- MultiParentCasper[F].blockDag
        allBlocksTopoSort <- dag.topoSort(0L)
        maybeBlock <- findBlockWithDeploy[F](
                       allBlocksTopoSort.flatten.reverse,
                       accountPublicKey,
                       timestamp
                     )
        blockQueryResponse <- maybeBlock.traverse(getFullBlockInfo[F])
      } yield
        blockQueryResponse.fold(
          BlockQueryResponse(
            status = s"Error: Failure to find block containing deploy signed by ${PrettyPrinter
              .buildString(accountPublicKey)} with timestamp ${timestamp.toString}"
          )
        )(
          blockInfo =>
            BlockQueryResponse(
              status = "Success",
              blockInfo = Some(blockInfo)
            )
        )

    MultiParentCasperRef.withCasper[F, BlockQueryResponse](
      casperResponse(_),
      errorMessage,
      BlockQueryResponse(status = s"Error: errorMessage").pure[F]
    )
  }

  private def findBlockWithDeploy[F[_]: MonadThrowable: Log: BlockStore](
      blockHashes: Vector[BlockHash],
      accountPublicKey: ByteString,
      timestamp: Long
  ): F[Option[Block]] =
    blockHashes.toStream
      .traverse(ProtoUtil.unsafeGetBlock[F](_))
      .map(blocks => blocks.find(ProtoUtil.containsDeploy(_, accountPublicKey, timestamp)))

  def getBlockInfoOpt[F[_]: MonadThrowable: Log: MultiParentCasperRef: SafetyOracle: BlockStore](
      blockHashBase16: String,
      full: Boolean = false
  ): F[Option[BlockInfo]] =
    unsafeWithCasper[F, Option[BlockInfo]]("Could not show block.") { implicit casper =>
      getByHashPrefix[F, BlockSummary](blockHashBase16)(
        BlockStore[F].getBlockSummary(_)
      ).flatMap { maybeSummary =>
        maybeSummary.fold(none[BlockInfo].pure[F]) { summary =>
          for {
            dag            <- MultiParentCasper[F].blockDag
            faultTolerance <- SafetyOracle[F].normalizedFaultTolerance(dag, summary.blockHash)
            initialFault <- MultiParentCasper[F].normalizedInitialFault(
                             ProtoUtil.weightMap(summary.getHeader)
                           )
            maybeBlockAndStats <- if (full) {
                                   BlockStore[F]
                                     .get(summary.blockHash)
                                     .map(_.get.getBlockMessage)
                                     .map { block =>
                                       val stats = BlockStatus
                                         .Stats()
                                         .withBlockSizeBytes(block.serializedSize)
                                         .withDeployErrorCount(
                                           block.getBody.deploys.count(_.isError)
                                         )
                                       (block, stats).some
                                     }
                                 } else {
                                   none[(Block, BlockStatus.Stats)].pure[F]
                                 }
            status = BlockStatus(
              faultTolerance = faultTolerance - initialFault,
              stats = maybeBlockAndStats.map(_._2)
            )
            result = BlockInfo()
              .withSummary(summary)
              .withStatus(status)
          } yield result.some
        }
      }
    }

  def getBlockInfo[F[_]: MonadThrowable: Log: MultiParentCasperRef: SafetyOracle: BlockStore](
      blockHashBase16: String,
      full: Boolean = false
  ): F[BlockInfo] =
    getBlockInfoOpt[F](blockHashBase16, full).flatMap(
      _.fold(
        MonadThrowable[F]
          .raiseError[BlockInfo](
            NotFound(s"Cannot find block matching hash $blockHashBase16")
          )
      )(_.pure[F])
    )

  /** Return block infos in the a slice of the DAG. Use `maxRank` 0 to get the top slice,
    * then we pass previous ranks to paginate backwards. */
  def getBlockInfos[F[_]: MonadThrowable: Log: MultiParentCasperRef: SafetyOracle: BlockStore](
      depth: Int,
      maxRank: Long = 0,
      full: Boolean = false
  ): F[List[BlockInfo]] =
    unsafeWithCasper[F, List[BlockInfo]]("Could not show blocks.") { implicit casper =>
      casper.blockDag flatMap { dag =>
        maxRank match {
          case 0 => dag.topoSortTail(depth)
          case r => dag.topoSort(endBlockNumber = r, startBlockNumber = r - depth + 1)
        }
      } handleErrorWith {
        case ex: StorageError =>
          MonadThrowable[F].raiseError(InvalidArgument(StorageError.errorMessage(ex)))
        case ex: IllegalArgumentException =>
          MonadThrowable[F].raiseError(InvalidArgument(ex.getMessage))
      } map { ranksOfHashes =>
        ranksOfHashes.flatten.reverse.map(h => Base16.encode(h.toByteArray))
      } flatMap { hashes =>
        hashes.toList.traverse(getBlockInfo[F](_, full))
      }
    }

  @deprecated("To be removed before devnet. Use `getBlockInfo`.", "0.4")
  def showBlock[F[_]: Monad: MultiParentCasperRef: Log: SafetyOracle: BlockStore](
      q: BlockQuery
  ): F[BlockQueryResponse] = {
    val errorMessage =
      "Could not show block."

    def casperResponse(implicit casper: MultiParentCasper[F]) =
      for {
        maybeBlock <- getByHashPrefix[F, Block](q.hash)(
                       BlockStore[F].get(_).map(_ flatMap (_.blockMessage))
                     )
        blockQueryResponse <- maybeBlock match {
                               case Some(block) =>
                                 for {
                                   blockInfo <- getFullBlockInfo[F](block)
                                 } yield
                                   BlockQueryResponse(
                                     status = "Success",
                                     blockInfo = Some(blockInfo)
                                   )
                               case None =>
                                 BlockQueryResponse(
                                   status = s"Error: Failure to find block with hash ${q.hash}"
                                 ).pure[F]
                             }
      } yield blockQueryResponse

    MultiParentCasperRef.withCasper[F, BlockQueryResponse](
      casperResponse(_),
      errorMessage,
      BlockQueryResponse(status = s"Error: $errorMessage").pure[F]
    )
  }

  private def getBlockInfo[A, F[_]: Monad: MultiParentCasper: SafetyOracle: BlockStore](
      block: Block,
      constructor: (
          Block,
          Long,
          Int,
          BlockHash,
          Long,
          BlockHash,
          Seq[BlockHash],
          Float,
          Float
      ) => F[A]
  ): F[A] =
    for {
      dag                      <- MultiParentCasper[F].blockDag
      header                   = block.getHeader
      protocolVersion          = header.protocolVersion
      deployCount              = header.deployCount
      postStateHash            = ProtoUtil.postStateHash(block)
      timestamp                = header.timestamp
      mainParent               = header.parentHashes.headOption.getOrElse(ByteString.EMPTY)
      parentsHashList          = header.parentHashes
      normalizedFaultTolerance <- SafetyOracle[F].normalizedFaultTolerance(dag, block.blockHash)
      initialFault             <- MultiParentCasper[F].normalizedInitialFault(ProtoUtil.weightMap(block))
      blockInfo <- constructor(
                    block,
                    protocolVersion,
                    deployCount,
                    postStateHash,
                    timestamp,
                    mainParent,
                    parentsHashList,
                    normalizedFaultTolerance,
                    initialFault
                  )
    } yield blockInfo

  private def getFullBlockInfo[F[_]: Monad: MultiParentCasper: SafetyOracle: BlockStore](
      block: Block
  ): F[BlockInfoWithTuplespace] =
    getBlockInfo[BlockInfoWithTuplespace, F](block, constructBlockInfo[F] _)

  private def getBlockInfoWithoutTuplespace[F[_]: Monad: MultiParentCasper: SafetyOracle: BlockStore](
      block: Block
  ): F[BlockInfoWithoutTuplespace] =
    getBlockInfo[BlockInfoWithoutTuplespace, F](block, constructBlockInfoWithoutTuplespace[F] _)

  private def constructBlockInfo[F[_]: Monad: MultiParentCasper: SafetyOracle: BlockStore](
      block: Block,
      protocolVersion: Long,
      deployCount: Int,
      postStateHash: BlockHash,
      timestamp: Long,
      mainParent: BlockHash,
      parentsHashList: Seq[BlockHash],
      normalizedFaultTolerance: Float,
      initialFault: Float
  ): F[BlockInfoWithTuplespace] =
    protocol
      .BlockInfo(
        blockHash = PrettyPrinter.buildStringNoLimit(block.blockHash),
        blockSize = block.serializedSize.toString,
        blockNumber = ProtoUtil.blockNumber(block),
        protocolVersion = protocolVersion,
        deployCount = deployCount,
        globalStateRootHash = PrettyPrinter.buildStringNoLimit(postStateHash),
        timestamp = timestamp,
        faultTolerance = normalizedFaultTolerance - initialFault,
        mainParentHash = PrettyPrinter.buildStringNoLimit(mainParent),
        parentsHashList = parentsHashList.map(PrettyPrinter.buildStringNoLimit),
        sender = PrettyPrinter.buildStringNoLimit(block.getHeader.validatorPublicKey),
        shardId = block.getHeader.chainId
      )
      .pure[F]

  private def constructBlockInfoWithoutTuplespace[F[_]: Monad: MultiParentCasper: SafetyOracle: BlockStore](
      block: Block,
      protocolVersion: Long,
      deployCount: Int,
      postStateHash: BlockHash,
      timestamp: Long,
      mainParent: BlockHash,
      parentsHashList: Seq[BlockHash],
      normalizedFaultTolerance: Float,
      initialFault: Float
  ): F[BlockInfoWithoutTuplespace] =
    BlockInfoWithoutTuplespace(
      blockHash = PrettyPrinter.buildStringNoLimit(block.blockHash),
      blockSize = block.serializedSize.toString,
      blockNumber = ProtoUtil.blockNumber(block),
      protocolVersion = protocolVersion,
      deployCount = deployCount,
      globalStateRootHash = PrettyPrinter.buildStringNoLimit(postStateHash),
      timestamp = timestamp,
      faultTolerance = normalizedFaultTolerance - initialFault,
      mainParentHash = PrettyPrinter.buildStringNoLimit(mainParent),
      parentsHashList = parentsHashList.map(PrettyPrinter.buildStringNoLimit),
      sender = PrettyPrinter.buildStringNoLimit(block.getHeader.validatorPublicKey)
    ).pure[F]

  private def getByHashPrefix[F[_]: Monad: MultiParentCasper: BlockStore, A](
      blockHashBase16: String
  )(f: ByteString => F[Option[A]]): F[Option[A]] =
    if (blockHashBase16.size == 64) {
      f(ByteString.copyFrom(Base16.decode(blockHashBase16)))
    } else {
      for {
        maybeHash <- BlockStore[F].findBlockHash { h =>
                      Base16.encode(h.toByteArray).startsWith(blockHashBase16)
                    }
        maybeA <- maybeHash.fold(none[A].pure[F])(f(_))
      } yield maybeA
    }

}
