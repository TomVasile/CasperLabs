package io.casperlabs.comm.discovery

import scala.collection.mutable
import scala.concurrent.duration._
import cats.implicits._
import cats.temp.par._
import io.casperlabs.catscontrib._
import Catscontrib._
import cats.effect._
import io.casperlabs.comm._
import io.casperlabs.metrics.Metrics
import io.casperlabs.shared._
import monix.eval.{TaskLift, TaskLike}
import monix.execution.Scheduler

object NodeDiscoveryImpl {
  def create[F[_]: Concurrent: Log: Time: Metrics: TaskLike: TaskLift: NodeAsk: Timer: Par](
      id: NodeIdentifier,
      port: Int,
      timeout: FiniteDuration
  )(
      init: Option[Node]
  )(implicit scheduler: Scheduler): Resource[F, NodeDiscovery[F]] = {
    val kademliaRpcResource = Resource.make(CachedConnections[F, KademliaConnTag].map {
      implicit cache =>
        new GrpcKademliaService(port, timeout)
    })(
      kRpc =>
        Concurrent[F]
          .attempt(kRpc.shutdown())
          .flatMap(
            _.fold(ex => Log[F].error("Failed to properly shutdown KademliaRPC", ex), _.pure[F])
          )
    )
    kademliaRpcResource.flatMap { implicit kRpc =>
      Resource.liftF(for {
        table <- PeerTable[F](id)
        knd   = new NodeDiscoveryImpl[F](id, table)
        _     <- init.fold(().pure[F])(knd.addNode)
      } yield knd)
    }
  }
}

private[discovery] class NodeDiscoveryImpl[F[_]: Sync: Log: Time: Metrics: KademliaService: Par](
    id: NodeIdentifier,
    table: PeerTable[F],
    alpha: Int = 3,
    k: Int = PeerTable.Redundancy
) extends NodeDiscovery[F] {
  private implicit val metricsSource: Metrics.Source =
    Metrics.Source(CommMetricsSource, "discovery.kademlia")

  // TODO inline usage
  private[discovery] def addNode(peer: Node): F[Unit] =
    for {
      _     <- table.updateLastSeen(peer)
      peers <- table.peersAscendingDistance
      _     <- Metrics[F].setGauge("peers", peers.length.toLong)
    } yield ()

  private def pingHandler(peer: Node): F[Unit] =
    addNode(peer) *> Metrics[F].incrementCounter("handle.ping")

  private def lookupHandler(peer: Node, id: NodeIdentifier): F[Seq[Node]] =
    for {
      peers <- table.lookup(id)
      _     <- Metrics[F].incrementCounter("handle.lookup")
      _     <- addNode(peer)
    } yield peers

  def discover: F[Unit] = {

    val initRPC = KademliaService[F].receive(pingHandler, lookupHandler)

    val findNew = for {
      _ <- Time[F].sleep(9.seconds)
      _ <- findMorePeers()
    } yield ()

    initRPC *> lookup(id) *> findNew.forever
  }

  private def findMorePeers(
      alpha: Int = 3,
      k: Int = PeerTable.Redundancy,
      bucketsToFill: Int = 5
  ): F[Unit] = {

    def generateId(distance: Int): NodeIdentifier = {
      val target       = id.key.to[mutable.ArrayBuffer] // Our key
      val byteIndex    = distance / 8
      val differentBit = 1 << (distance % 8)
      target(byteIndex) = (target(byteIndex) ^ differentBit).toByte // A key at a distance dist from me
      NodeIdentifier(target.toList)
    }

    for {
      targetIds <- table.sparseness.map(_.take(bucketsToFill).map(generateId).toList)
      _         <- targetIds.traverse(lookup)
    } yield ()
  }

  def lookup(toLookup: NodeIdentifier): F[Option[Node]] = {
    def loop(successQueriesN: Int, alreadyQueried: Set[NodeIdentifier], shortlist: Seq[Node])(
        maybeClosestPeerNode: Option[Node]
    ): F[Option[Node]] =
      if (shortlist.isEmpty || successQueriesN >= k) {
        maybeClosestPeerNode.pure[F]
      } else {
        val (callees, rest) = shortlist.toList.splitAt(alpha)
        for {
          responses <- callees.parTraverse { callee =>
                        for {
                          maybeNodes <- KademliaService[F].lookup(toLookup, callee)
                          _          <- maybeNodes.fold(().pure[F])(_ => addNode(callee))
                        } yield (callee, maybeNodes)
                      }
          newAlreadyQueried = alreadyQueried ++ responses.collect {
            case (callee, Some(_)) => NodeIdentifier(callee.id)
          }.toSet
          returnedPeers = responses.flatMap(_._2.toList.flatten).distinct
          recursion = loop(
            successQueriesN + responses.count(_._2.nonEmpty),
            newAlreadyQueried,
            rest ::: returnedPeers.filterNot(p => newAlreadyQueried(NodeIdentifier(p.id)))
          ) _
          maybeNewClosestPeerNode = if (returnedPeers.nonEmpty)
            returnedPeers.minBy(p => PeerTable.xorDistance(toLookup.asByteString, p.id)).some
          else None
          res <- (maybeNewClosestPeerNode, maybeClosestPeerNode) match {
                  case (x @ Some(_), None) => recursion(x)
                  case (x @ Some(newClosestPeerNode), Some(closestPeerNode))
                      if PeerTable.xorDistance(toLookup.asByteString, newClosestPeerNode.id) <
                        PeerTable.xorDistance(toLookup.asByteString, closestPeerNode.id) =>
                    recursion(x)
                  case _ => maybeClosestPeerNode.pure[F]
                }
        } yield res
      }

    for {
      shortlist <- table.lookup(toLookup).map(_.take(alpha))
      closestNode <- shortlist.headOption
                      .filter(p => NodeIdentifier(p.id) == toLookup)
                      .fold(loop(0, Set(id), shortlist)(None))(_.some.pure[F])

    } yield closestNode
  }

  override def alivePeersAscendingDistance: F[List[Node]] =
    table.peersAscendingDistance.flatMap { peers =>
      peers.parFlatTraverse { peer =>
        KademliaService[F].ping(peer).map(success => if (success) List(peer) else Nil)
      }
    }
}
