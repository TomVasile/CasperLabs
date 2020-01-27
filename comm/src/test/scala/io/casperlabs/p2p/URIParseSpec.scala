package io.casperlabs.p2p

import com.google.protobuf.ByteString
import io.casperlabs.comm.discovery.Node
import io.casperlabs.comm.discovery.NodeUtils._
import org.scalatest._

class URIParseSpec extends FlatSpec with Matchers {
  def badAddressError(s: String) =
    Left(s"bad address: $s")

  "A well formed casperlabs URI" should "parse into a PeerNode" in {
    val uri =
      "casperlabs://abcdef@localhost?protocol=12345&discovery=12346"

    Node.fromAddress(uri) should be(
      Right(
        NodeWithoutChainId(
          Node(
            ByteString.copyFrom(Array(0xAB.toByte, 0xCD.toByte, 0xEF.toByte)),
            "localhost",
            12345,
            12346,
            ByteString.EMPTY
          )
        )
      )
    )
  }

  "A non-casperlabs URI" should "parse as an error" in {
    val uri = "http://foo.bar.baz/quux"
    Node.fromAddress(uri) should be(badAddressError(uri))
  }

  "A URI without protocol" should "parse as an error" in {
    val uri = "abcde@localhost?protocol=12345&discovery=12346"
    Node.fromAddress(uri) should be(badAddressError(uri))
  }

  "An casperlabs URI with non-integral port" should "parse as an error" in {
    val uri = "casperlabs://abcde@localhost:smtp"
    Node.fromAddress(uri) should be(badAddressError(uri))
  }

  "An casperlabs URI without a key" should "parse as an error" in {
    val uri = "casperlabs://localhost?protocol=12345&discovery=12346"
    Node.fromAddress(uri) should be(badAddressError(uri))
  }
}
