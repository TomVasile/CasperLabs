package io.casperlabs.node

import java.nio.file.Path

import cats._
import cats.data._
import cats.effect._
import cats.effect.concurrent.{Ref, Semaphore}
import cats.mtl.{FunctorRaise, MonadState}
import cats.syntax.applicative._
import cats.syntax.apply._
import cats.syntax.flatMap._
import cats.syntax.functor._
import cats.syntax.show._
import cats.temp.par.Par
import com.olegpy.meow.effects._
import io.casperlabs.blockstorage.util.fileIO.IOError
import io.casperlabs.blockstorage.util.fileIO.IOError.RaiseIOError
import io.casperlabs.blockstorage.{
  BlockDagFileStorage,
  BlockDagStorage,
  BlockStore,
  CachingBlockStore,
  FileLMDBIndexBlockStore
}
import io.casperlabs.casper.MultiParentCasperRef.MultiParentCasperRef
import io.casperlabs.casper._
import io.casperlabs.casper.validation.{Validation, ValidationImpl}
import io.casperlabs.catscontrib.Catscontrib._
import io.casperlabs.catscontrib.TaskContrib._
import io.casperlabs.catscontrib._
import io.casperlabs.catscontrib.effect.implicits.{syncId, taskLiftEitherT}
import io.casperlabs.comm._
import io.casperlabs.comm.discovery.NodeDiscovery._
import io.casperlabs.comm.discovery.NodeUtils._
import io.casperlabs.comm.discovery._
import io.casperlabs.comm.grpc.SslContexts
import io.casperlabs.comm.rp.Connect.RPConfState
import io.casperlabs.comm.rp._
import io.casperlabs.metrics.Metrics
import io.casperlabs.node.api.graphql.FinalizedBlocksStream
import io.casperlabs.node.configuration.Configuration
import io.casperlabs.shared._
import io.casperlabs.smartcontracts.{ExecutionEngineService, GrpcExecutionEngineService}
import io.netty.handler.ssl.ClientAuth
import monix.eval.{Task, TaskLike}
import monix.execution.Scheduler
import org.flywaydb.core.Flyway
import org.flywaydb.core.api.Location

import scala.concurrent.duration._

class NodeRuntime private[node] (
    conf: Configuration,
    id: NodeIdentifier
)(implicit log: Log[Task], scheduler: Scheduler) {

  private[this] val loopScheduler =
    Scheduler.fixedPool("loop", 4, reporter = UncaughtExceptionHandler)
  private[this] val blockingScheduler =
    Scheduler.cached("blocking-io", 4, 64, reporter = UncaughtExceptionHandler)
  private implicit val concurrentEffectForEffect: ConcurrentEffect[Effect] =
    catsConcurrentEffectForEffect(
      scheduler
    )

  implicit val raiseIOError: RaiseIOError[Effect] = IOError.raiseIOErrorThroughSync[Effect]

  // intra-node gossiping port.
  private val port           = conf.server.port
  private val kademliaPort   = conf.server.kademliaPort
  private val blockstorePath = conf.server.dataDir.resolve("blockstore")
  private val dagStoragePath = conf.server.dataDir.resolve("dagstorage")

  /**
    * Main node entry. It will:
    * 1. set up configurations
    * 2. create instances of typeclasses
    * 3. run the node program.
    */
  // TODO: Resolve scheduler chaos in Runtime, RuntimeManager and CasperPacketHandler

  val main: Effect[Unit] = {
    val rpConfState = (for {
      local     <- localPeerNode[Task]
      bootstrap <- initPeer[Task]
      conf      <- rpConf[Task](local, bootstrap)
    } yield conf).toEffect

    implicit val logEff: Log[Effect] = Log.eitherTLog(Monad[Task], log)

    implicit val logId: Log[Id]         = Log.logId
    implicit val metricsId: Metrics[Id] = diagnostics.effects.metrics[Id](syncId)
    implicit val parForEff: Par[Effect] = catsParForEffect
    implicit val filesApiEff            = FilesAPI.create[Effect](Sync[Effect], logEff)

    // SSL context to use for the public facing API.
    val maybeApiSslContext = Option(conf.tls.readCertAndKey).filter(_ => conf.grpc.useTls).map {
      case (cert, key) =>
        SslContexts.forServer(cert, key, ClientAuth.NONE)
    }

    rpConfState >>= (_.runState { implicit state =>
      implicit val metrics     = diagnostics.effects.metrics[Task]
      implicit val nodeMetrics = diagnostics.effects.nodeCoreMetrics[Task]
      implicit val jvmMetrics  = diagnostics.effects.jvmMetrics[Task]
      implicit val nodeAsk     = eitherTApplicativeAsk(effects.peerNodeAsk(state))

      implicit val metricsEff: Metrics[Effect] =
        Metrics.eitherT[CommError, Task](Monad[Task], metrics)
      val resources = for {
        implicit0(executionEngineService: ExecutionEngineService[Effect]) <- GrpcExecutionEngineService[
                                                                              Effect
                                                                            ](
                                                                              conf.grpc.socket,
                                                                              conf.server.maxMessageSize
                                                                            )

        maybeBootstrap <- Resource.liftF(initPeer[Effect])

        implicit0(finalizedBlocksStream: FinalizedBlocksStream[Effect]) <- Resource.liftF(
                                                                            FinalizedBlocksStream
                                                                              .of[Effect]
                                                                          )

        implicit0(nodeDiscovery: NodeDiscovery[Task]) <- effects.nodeDiscovery(
                                                          id,
                                                          kademliaPort,
                                                          conf.server.defaultTimeout,
                                                          conf.server.useGossiping,
                                                          conf.server.relayFactor,
                                                          conf.server.relaySaturation
                                                        )(
                                                          maybeBootstrap
                                                        )(
                                                          blockingScheduler,
                                                          effects.peerNodeAsk,
                                                          log,
                                                          metrics
                                                        )
        _ <- Resource.liftF(runRdmbsMigrations(conf.server.dataDir))

        implicit0(blockStore: BlockStore[Effect]) <- FileLMDBIndexBlockStore[Effect](
                                                      conf.server.dataDir,
                                                      blockstorePath,
                                                      100L * 1024L * 1024L * 4096L
                                                    )(
                                                      Concurrent[Effect],
                                                      logEff,
                                                      raiseIOError,
                                                      metricsEff
                                                    ) evalMap { underlying =>
                                                      CachingBlockStore[Effect](
                                                        underlying,
                                                        maxSizeBytes =
                                                          conf.blockstorage.cacheMaxSizeBytes
                                                      )(
                                                        Sync[Effect],
                                                        metricsEff
                                                      )
                                                    }

        implicit0(blockDagStorage: BlockDagStorage[Effect]) <- BlockDagFileStorage[Effect](
                                                                dagStoragePath,
                                                                conf.blockstorage.latestMessagesLogMaxSizeFactor,
                                                                blockStore
                                                              )(
                                                                Concurrent[Effect],
                                                                logEff,
                                                                raiseIOError,
                                                                metricsEff
                                                              )

        _ <- Resource.liftF {
              Task
                .delay {
                  log.info("Cleaning block storage ...")
                  blockStore.clear() *> blockDagStorage.clear()
                }
                .whenA(conf.server.cleanBlockStorage)
                .toEffect
            }

        implicit0(raise: FunctorRaise[Effect, InvalidBlock]) = validation
          .raiseValidateErrorThroughApplicativeError[Effect]
        implicit0(validationEff: Validation[Effect]) = new ValidationImpl[Effect]

        // TODO: Only a loop started with the TransportLayer keeps filling this up,
        // so if we use the GossipService it's going to stay empty. The diagnostics
        // should use NodeDiscovery instead.
        implicit0(connectionsCell: Connect.ConnectionsCell[Task]) <- Resource.liftF(
                                                                      effects.rpConnections.toEffect
                                                                    )

        implicit0(multiParentCasperRef: MultiParentCasperRef[Effect]) <- Resource.liftF(
                                                                          MultiParentCasperRef
                                                                            .of[Effect]
                                                                        )

        implicit0(safetyOracle: FinalityDetector[Effect]) = new FinalityDetectorInstancesImpl[
          Effect
        ]()(
          Monad[Effect],
          logEff
        )

        blockApiLock <- Resource.liftF(Semaphore[Effect](1))

        // For now just either starting the auto-proposer or not, but ostensibly we
        // could pass it the flag to run or not and also wire it into the ControlService
        // so that the operator can turn it on/off on the fly.
        _ <- AutoProposer[Effect](
              checkInterval = conf.casper.autoProposeCheckInterval,
              maxInterval = conf.casper.autoProposeMaxInterval,
              maxCount = conf.casper.autoProposeMaxCount,
              blockApiLock = blockApiLock
            )(
              Concurrent[Effect],
              Time[Effect],
              logEff,
              metricsEff,
              multiParentCasperRef
            ).whenA(conf.casper.autoProposeEnabled)

        _ <- api.Servers
              .internalServersR(
                conf.grpc.portInternal,
                conf.server.maxMessageSize,
                blockingScheduler,
                blockApiLock,
                maybeApiSslContext
              )

        _ <- api.Servers.externalServersR[Effect](
              conf.grpc.portExternal,
              conf.server.maxMessageSize,
              blockingScheduler,
              blockApiLock,
              conf.casper.ignoreDeploySignature,
              maybeApiSslContext
            )

        _ <- api.Servers.httpServerR[Effect](
              conf.server.httpPort,
              conf,
              id,
              blockingScheduler
            )

        _ <- if (conf.server.useGossiping) {
              casper.gossiping.apply[Effect](
                port,
                conf,
                blockingScheduler
              )
            } else {
              casper.transport.apply(
                port,
                conf,
                blockingScheduler
              )
            }

      } yield (nodeDiscovery, multiParentCasperRef)

      resources.allocated flatMap {
        case ((nodeDiscovery, multiParentCasperRef), release) =>
          handleUnrecoverableErrors {
            nodeProgram(state, nodeDiscovery, multiParentCasperRef, release)
          }
      }
    })
  }

  private def runRdmbsMigrations(serverDataDir: Path): Effect[Unit] =
    Task.delay {
      val db = serverDataDir.resolve("sqlite.db").toString
      val conf =
        Flyway
          .configure()
          .dataSource(s"jdbc:sqlite:$db", "", "")
          .locations(new Location("classpath:db/migration"))
      val flyway = conf.load()
      flyway.migrate()
      ()
    }.toEffect

  /** Start periodic tasks as fibers. They'll automatically stop during shutdown. */
  private def nodeProgram(
      implicit
      rpConfState: RPConfState[Task],
      nodeDiscovery: NodeDiscovery[Task],
      multiParentCasperRef: MultiParentCasperRef[Effect],
      release: Effect[Unit]
  ): Effect[Unit] = {

    val peerNodeAsk = effects.peerNodeAsk(rpConfState)
    val time        = effects.time

    val info: Effect[Unit] =
      if (conf.casper.standalone) Log[Effect].info(s"Starting stand-alone node.")
      else
        Log[Effect].info(
          s"Starting node that will bootstrap from ${conf.server.bootstrap.map(_.show).getOrElse("n/a")}"
        )

    val fetchLoop: Effect[Unit] =
      for {
        casper <- multiParentCasperRef.get
        _      <- casper.fold(().pure[Effect])(_.fetchDependencies)
        _      <- time.sleep(30.seconds).toEffect
      } yield ()

    for {
      _       <- addShutdownHook(release).toEffect
      _       <- info
      _       <- Task.defer(fetchLoop.forever.value).executeOn(loopScheduler).start.toEffect
      host    <- peerNodeAsk.ask.toEffect.map(_.host)
      address = s"casperlabs://$id@$host?protocol=$port&discovery=$kademliaPort"
      _       <- Log[Effect].info(s"Listening for traffic on $address.")
      // This loop will keep the program from exiting until shutdown is initiated.
      _ <- NodeDiscovery[Task].discover.attemptAndLog.executeOn(loopScheduler).toEffect
    } yield ()
  }

  private def shutdown(release: Effect[Unit]): Unit = {
    // Everything has been moved to Resources.
    val task = for {
      _ <- log.info("Shutting down...")
      _ <- release.value
      _ <- log.info("Goodbye.")
    } yield ()
    // Run the release synchronously so that we can see the final message.
    task.unsafeRunSync(scheduler)
  }

  private def addShutdownHook(release: Effect[Unit]): Task[Unit] =
    Task.delay(
      sys.addShutdownHook(shutdown(release))
    )

  /**
    * Handles unrecoverable errors in program. Those are errors that should not happen in properly
    * configured environment and they mean immediate termination of the program
    */
  private def handleUnrecoverableErrors(prog: Effect[Unit]): Effect[Unit] =
    EitherT[Task, CommError, Unit](
      prog.value
        .onErrorHandleWith { th =>
          log.error("Caught unhandable error. Exiting. Stacktrace below.") *> Task.delay {
            th.printStackTrace()
          }
        } *> Task.delay(System.exit(1)).as(Right(()))
    )

  private def rpConf[F[_]: Sync](local: Node, maybeBootstrap: Option[Node]) =
    Ref.of[F, RPConf](
      RPConf(
        local,
        maybeBootstrap,
        conf.server.defaultTimeout,
        ClearConnectionsConf(
          conf.server.maxNumOfConnections,
          // TODO read from conf
          numOfConnectionsPinged = 10
        )
      )
    )

  private def localPeerNode[F[_]: Sync: Log] =
    WhoAmI
      .fetchLocalPeerNode[F](
        conf.server.host,
        conf.server.port,
        conf.server.kademliaPort,
        conf.server.noUpnp,
        id
      )

  private def initPeer[F[_]: MonadThrowable]: F[Option[Node]] =
    conf.server.bootstrap match {
      case None if !conf.casper.standalone =>
        MonadThrowable[F].raiseError(
          new java.lang.IllegalStateException(
            "Not in standalone mode but there's no bootstrap configured!"
          )
        )
      case other =>
        other.pure[F]
    }

}

object NodeRuntime {
  def apply(
      conf: Configuration
  )(implicit scheduler: Scheduler, log: Log[Task]): Effect[NodeRuntime] =
    for {
      id      <- NodeEnvironment.create(conf)
      runtime <- Task.delay(new NodeRuntime(conf, id)).toEffect
    } yield runtime
}
