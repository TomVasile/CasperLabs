package io.casperlabs.blockstorage

import cats.Applicative
import cats.implicits._
import com.google.protobuf.ByteString
import io.casperlabs.blockstorage.StorageError.StorageIOErr
import io.casperlabs.casper.protocol.BlockMessage

import scala.language.higherKinds

trait BlockStore[F[_]] {
  import BlockStore.BlockHash

  def put(blockHash: BlockHash, blockMessage: BlockMessage): F[StorageIOErr[Unit]] =
    put((blockHash, blockMessage))

  def get(blockHash: BlockHash): F[Option[BlockMessage]]

  def find(p: BlockHash => Boolean): F[Seq[(BlockHash, BlockMessage)]]

  def put(f: => (BlockHash, BlockMessage)): F[StorageIOErr[Unit]]

  def apply(blockHash: BlockHash)(implicit applicativeF: Applicative[F]): F[BlockMessage] =
    get(blockHash).map(_.get)

  def contains(blockHash: BlockHash)(implicit applicativeF: Applicative[F]): F[Boolean] =
    get(blockHash).map(_.isDefined)

  def checkpoint(): F[Unit]

  def clear(): F[StorageIOErr[Unit]]

  def close(): F[StorageIOErr[Unit]]
}

object BlockStore {

  def apply[F[_]](implicit ev: BlockStore[F]): BlockStore[F] = ev

  type BlockHash = ByteString

}
