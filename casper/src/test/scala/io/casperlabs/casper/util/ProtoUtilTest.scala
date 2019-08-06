package io.casperlabs.casper.util

import com.google.protobuf.ByteString
import io.casperlabs.casper.Estimator.BlockHash
import io.casperlabs.casper._
import io.casperlabs.casper.consensus._, Block.Justification
import org.scalacheck.{Arbitrary, Gen}
import org.scalacheck.Arbitrary.arbitrary
import org.scalacheck.Gen.listOfN
import org.scalatest._
import org.scalatest.prop.GeneratorDrivenPropertyChecks

class ProtoUtilTest extends FlatSpec with Matchers with GeneratorDrivenPropertyChecks {

  val blockHashGen: Gen[BlockHash] = for {
    byteArray <- listOfN(32, arbitrary[Byte])
  } yield ByteString.copyFrom(byteArray.toArray)

  implicit val arbitraryHash: Arbitrary[BlockHash] = Arbitrary(blockHashGen)

  val justificationGen: Gen[Justification] = for {
    latestBlockHash <- arbitrary[BlockHash]
  } yield (Justification().withLatestBlockHash(latestBlockHash))

  implicit val arbitraryJustification: Arbitrary[Justification] = Arbitrary(justificationGen)

  val blockElementGen: Gen[Block] =
    for {
      hash            <- arbitrary[BlockHash]
      version         <- arbitrary[Long]
      timestamp       <- arbitrary[Long]
      parentsHashList <- arbitrary[Seq[BlockHash]]
      justifications  <- arbitrary[Seq[Justification]]
    } yield
      Block(blockHash = hash)
        .withHeader(
          Block
            .Header()
            .withParentHashes(parentsHashList)
            .withJustifications(justifications)
            .withProtocolVersion(version)
            .withTimestamp(timestamp)
        )

  implicit val arbitraryBlock: Arbitrary[Block] = Arbitrary(blockElementGen)

  "dependenciesHashesOf" should "return hashes of all justifications and parents of a block" in {
    forAll(blockElementGen) { blockElement =>
      val result = ProtoUtil.dependenciesHashesOf(blockElement)
      val justificationsHashes = blockElement.getHeader.justifications.map(
        _.latestBlockHash
      )
      val parentsHashes = blockElement.getHeader.parentHashes
      result should contain allElementsOf (justificationsHashes)
      result should contain allElementsOf (parentsHashes)
      result should contain theSameElementsAs ((justificationsHashes ++ parentsHashes).toSet)
    }
  }
}
