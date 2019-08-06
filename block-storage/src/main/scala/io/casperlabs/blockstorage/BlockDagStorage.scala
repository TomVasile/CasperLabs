package io.casperlabs.blockstorage

import com.google.protobuf.ByteString
import io.casperlabs.blockstorage.BlockDagRepresentation.Validator
import io.casperlabs.blockstorage.BlockStore.BlockHash
import io.casperlabs.casper.consensus.Block
import io.casperlabs.metrics.{Metered, Metrics}

trait BlockDagStorage[F[_]] {
  def getRepresentation: F[BlockDagRepresentation[F]]
  def insert(block: Block): F[BlockDagRepresentation[F]]
  def checkpoint(): F[Unit]
  def clear(): F[Unit]
  def close(): F[Unit]
}

object BlockDagStorage {
  trait MeteredBlockDagStorage[F[_]] extends BlockDagStorage[F] with Metered[F] {

    abstract override def getRepresentation: F[BlockDagRepresentation[F]] =
      incAndMeasure("representation", super.getRepresentation)

    abstract override def insert(block: Block): F[BlockDagRepresentation[F]] =
      incAndMeasure("insert", super.insert(block))

    abstract override def checkpoint(): F[Unit] =
      incAndMeasure("checkpoint", super.checkpoint())
  }

  def apply[F[_]](implicit B: BlockDagStorage[F]): BlockDagStorage[F] = B
}

trait BlockDagRepresentation[F[_]] {
  def children(blockHash: BlockHash): F[Option[Set[BlockHash]]]
  def lookup(blockHash: BlockHash): F[Option[BlockMetadata]]
  def contains(blockHash: BlockHash): F[Boolean]

  /** Return the ranks of blocks in the DAG between start and end, inclusive. */
  def topoSort(startBlockNumber: Long, endBlockNumber: Long): F[Vector[Vector[BlockHash]]]

  /** Return ranks of blocks in the DAG from a start index to the end. */
  def topoSort(startBlockNumber: Long): F[Vector[Vector[BlockHash]]]

  def topoSortTail(tailLength: Int): F[Vector[Vector[BlockHash]]]
  def deriveOrdering(startBlockNumber: Long): F[Ordering[BlockMetadata]]
  def latestMessageHash(validator: Validator): F[Option[BlockHash]]
  def latestMessage(validator: Validator): F[Option[BlockMetadata]]
  def latestMessageHashes: F[Map[Validator, BlockHash]]
  def latestMessages: F[Map[Validator, BlockMetadata]]
}

object BlockDagRepresentation {
  type Validator = ByteString
}
