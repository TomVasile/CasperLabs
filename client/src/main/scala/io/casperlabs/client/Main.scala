package io.casperlabs.client

import java.nio.file.Files

import cats.effect.{Sync, Timer}
import cats.temp.par._
import com.google.protobuf.ByteString
import io.casperlabs.client.configuration._
import io.casperlabs.crypto.codec.Base16
import io.casperlabs.shared.{FilesAPI, Log, UncaughtExceptionHandler}
import monix.eval.Task
import monix.execution.Scheduler

object Main {

  implicit val log: Log[Task] = Log.log

  def main(args: Array[String]): Unit = {
    implicit val scheduler: Scheduler = Scheduler.computation(
      Math.max(java.lang.Runtime.getRuntime.availableProcessors(), 2),
      "node-runner",
      reporter = UncaughtExceptionHandler
    )

    val exec =
      for {
        maybeConf <- Task(Configuration.parse(args))
        _ <- maybeConf.fold(Log[Task].error("Couldn't parse CLI args into configuration")) {
              case (conn, conf) =>
                implicit val deployService: GrpcDeployService = new GrpcDeployService(conn)
                implicit val filesAPI: FilesAPI[Task]         = FilesAPI.create[Task]
                program[Task](conf).doOnFinish(_ => Task(deployService.close()))
            }
      } yield ()

    exec.runSyncUnsafe()
  }

  def program[F[_]: Sync: DeployService: Timer: FilesAPI: Log: Par](
      configuration: Configuration
  ): F[Unit] =
    configuration match {
      case ShowBlock(hash)   => DeployRuntime.showBlock(hash)
      case ShowDeploy(hash)  => DeployRuntime.showDeploy(hash)
      case ShowDeploys(hash) => DeployRuntime.showDeploys(hash)
      case ShowBlocks(depth) => DeployRuntime.showBlocks(depth)
      case Unbond(
          amount,
          nonce,
          contractCode,
          privateKey
          ) =>
        DeployRuntime.unbond(
          amount,
          nonce,
          contractCode,
          privateKey
        )
      case Bond(
          amount,
          nonce,
          contractCode,
          privateKey
          ) =>
        DeployRuntime.bond(
          amount,
          nonce,
          contractCode,
          privateKey
        )
      case Transfer(
          amount,
          recipientPublicKeyBase64,
          nonce,
          contractCode,
          privateKey
          ) =>
        DeployRuntime.transferCLI(
          nonce,
          contractCode,
          privateKey,
          recipientPublicKeyBase64,
          amount
        )
      case MakeDeploy(
          from,
          nonce,
          sessionCode,
          paymentCode,
          gasPrice,
          deployPath
          ) => {
        val deploy = DeployRuntime.makeDeploy(
          ByteString.copyFrom(Base16.decode(from)),
          nonce,
          gasPrice,
          Files.readAllBytes(sessionCode.toPath),
          Array.emptyByteArray,
          Files.readAllBytes(paymentCode.toPath)
        )
        DeployRuntime.saveDeploy(deploy, deployPath)
      }

      case Deploy(deploy) =>
        DeployRuntime.deploy(deploy)

      case Sign(deploy, signedDeployOut, publicKey, privateKey) =>
        DeployRuntime.sign(deploy, signedDeployOut, publicKey, privateKey)

      case Propose =>
        DeployRuntime.propose()

      case VisualizeDag(depth, showJustificationLines, out, streaming) =>
        DeployRuntime.visualizeDag(depth, showJustificationLines, out, streaming)

      case Query(hash, keyType, keyValue, path) =>
        DeployRuntime.queryState(hash, keyType, keyValue, path)

      case Balance(address, blockHash) =>
        DeployRuntime.balance(address, blockHash)
    }
}
