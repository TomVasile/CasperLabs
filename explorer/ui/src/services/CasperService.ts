import { grpc } from '@improbable-eng/grpc-web';
import { CasperService as GrpcCasperService } from '../grpc/io/casperlabs/node/api/casper_pb_service';
import {
  DeployInfo,
  BlockInfo
} from '../grpc/io/casperlabs/casper/consensus/info_pb';
import {
  GetDeployInfoRequest,
  StreamBlockInfosRequest,
  StateQuery,
  GetBlockStateRequest,
  GetBlockInfoRequest,
  StreamBlockDeploysRequest
} from '../grpc/io/casperlabs/node/api/casper_pb';
import { encodeBase16 } from '../lib/Conversions';
import { GrpcError } from './Errors';
import {
  Value as StateValue,
  Key
} from '../grpc/io/casperlabs/casper/consensus/state_pb';
import { ByteArrayArg } from '../lib/Serialization';
import { Block } from '../grpc/io/casperlabs/casper/consensus/consensus_pb';

export default class CasperService {
  constructor(
    // Point at either at a URL on a different port where grpcwebproxy is listening,
    // or use nginx to serve the UI files, the API and gRPC all on the same port without CORS.
    private url: string
  ) {}

  getDeployInfo(deployHash: ByteArray): Promise<DeployInfo> {
    return new Promise<DeployInfo>((resolve, reject) => {
      const request = new GetDeployInfoRequest();
      request.setDeployHashBase16(encodeBase16(deployHash));

      grpc.unary(GrpcCasperService.GetDeployInfo, {
        host: this.url,
        request: request,
        onEnd: res => {
          if (res.status === grpc.Code.OK) {
            resolve(res.message as DeployInfo);
          } else {
            reject(new GrpcError(res.status, res.statusMessage));
          }
        }
      });
    });
  }

  /** Return the block info including statistics. */
  getBlockInfo(
    blockHash: ByteArray | string,
    view?: 0 | 1
  ): Promise<BlockInfo> {
    return new Promise<BlockInfo>((resolve, reject) => {
      // The API supports prefixes, which may not have even number of characters.
      const hashBase16 =
        typeof blockHash === 'string' ? blockHash : encodeBase16(blockHash);
      const request = new GetBlockInfoRequest();
      request.setBlockHashBase16(hashBase16);
      request.setView(view === undefined ? BlockInfo.View.FULL : view);

      grpc.unary(GrpcCasperService.GetBlockInfo, {
        host: this.url,
        request: request,
        onEnd: res => {
          if (res.status === grpc.Code.OK) {
            resolve(res.message as BlockInfo);
          } else {
            reject(new GrpcError(res.status, res.statusMessage));
          }
        }
      });
    });
  }

  getBlockInfos(depth: number, maxRank?: number): Promise<BlockInfo[]> {
    return new Promise<BlockInfo[]>((resolve, reject) => {
      const request = new StreamBlockInfosRequest();
      request.setDepth(depth);
      request.setMaxRank(maxRank || 0);

      let blocks: BlockInfo[] = [];

      grpc.invoke(GrpcCasperService.StreamBlockInfos, {
        host: this.url,
        request: request,
        onMessage: msg => {
          blocks.push(msg as BlockInfo);
        },
        onEnd: (code, message) => {
          if (code === grpc.Code.OK) {
            resolve(blocks);
          } else {
            reject(new GrpcError(code, message));
          }
        }
      });
    });
  }

  getBlockDeploys(blockHash: ByteArray): Promise<Block.ProcessedDeploy[]> {
    return new Promise<Block.ProcessedDeploy[]>((resolve, reject) => {
      const request = new StreamBlockDeploysRequest();
      request.setBlockHashBase16(encodeBase16(blockHash));

      let deploys: Block.ProcessedDeploy[] = [];

      grpc.invoke(GrpcCasperService.StreamBlockDeploys, {
        host: this.url,
        request: request,
        onMessage: msg => {
          deploys.push(msg as Block.ProcessedDeploy);
        },
        onEnd: (code, message) => {
          if (code === grpc.Code.OK) {
            resolve(deploys);
          } else {
            reject(new GrpcError(code, message));
          }
        }
      });
    });
  }

  /** Get one of the blocks from the last rank. */
  getLatestBlockInfo(): Promise<BlockInfo> {
    return new Promise<BlockInfo>((resolve, reject) => {
      const request = new StreamBlockInfosRequest();
      request.setDepth(1);

      // For now just return any block, but ideally we should be looking at the fork choice tip.
      let resolved = false;

      grpc.invoke(GrpcCasperService.StreamBlockInfos, {
        host: this.url,
        request: request,
        onMessage: msg => {
          if (!resolved) {
            resolved = true;
            resolve(msg as BlockInfo);
          }
        },
        onEnd: (code, message) => {
          if (code !== grpc.Code.OK && !resolved) {
            reject(new GrpcError(code, message));
          }
        }
      });
    });
  }

  getBlockState(blockHash: BlockHash, query: StateQuery): Promise<StateValue> {
    return new Promise<StateValue>((resolve, reject) => {
      const request = new GetBlockStateRequest();
      request.setBlockHashBase16(encodeBase16(blockHash));
      request.setQuery(query);

      grpc.unary(GrpcCasperService.GetBlockState, {
        host: this.url,
        request: request,
        onEnd: res => {
          if (res.status === grpc.Code.OK) {
            resolve(res.message as StateValue);
          } else {
            reject(new GrpcError(res.status, res.statusMessage));
          }
        }
      });
    });
  }

  /** Get the reference to the balance so we can cache it.
   *  Returns `undefined` if the account doesn't exist yet.
   */
  async getAccountBalanceUref(
    blockHash: BlockHash,
    accountPublicKey: ByteArray
  ): Promise<Key.URef | undefined> {
    try {
      const accountQuery = QueryAccount(accountPublicKey);

      const account = await this.getBlockState(blockHash, accountQuery).then(
        res => res.getAccount()!
      );

      const mintPublic = account
        .getKnownUrefsList()
        .find(x => x.getName() === 'mint')!;

      const mintQuery = QueryUref(mintPublic.getKey()!.getUref()!);

      const mintPrivate = await this.getBlockState(blockHash, mintQuery).then(
        res => res.getKey()!.getUref()!
      );

      const localKeyQuery = QueryLocalKey(
        mintPrivate.getUref_asU8(),
        ByteArrayArg(account.getPurseId()!.getUref_asU8())
      );

      const balanceUref = await this.getBlockState(
        blockHash,
        localKeyQuery
      ).then(res => res.getKey()!.getUref()!);

      return balanceUref;
    } catch (err) {
      if (err instanceof GrpcError) {
        if (
          err.code === grpc.Code.InvalidArgument &&
          err.message.indexOf('Key') > -1
        ) {
          // The account doesn't exist yet.
          return undefined;
        }
      }
      throw err;
    }
  }

  async getAccountBalance(
    blockHash: BlockHash,
    balanceUref: Key.URef
  ): Promise<number> {
    const balanceQuery = QueryUref(balanceUref);
    const balance = await this.getBlockState(blockHash, balanceQuery).then(
      res => res.getBigInt()!
    );
    return Number(balance.getValue());
  }
}

const QueryAccount = (accountPublicKey: ByteArray) => {
  const query = new StateQuery();
  query.setKeyVariant(StateQuery.KeyVariant.ADDRESS);
  query.setKeyBase16(encodeBase16(accountPublicKey));
  return query;
};

const QueryUref = (uref: Key.URef) => {
  const query = new StateQuery();
  query.setKeyVariant(StateQuery.KeyVariant.UREF);
  query.setKeyBase16(encodeBase16(uref.getUref_asU8()));
  return query;
};

const QueryLocalKey = (seed: ByteArray, bytes: ByteArray) => {
  const query = new StateQuery();
  query.setKeyVariant(StateQuery.KeyVariant.LOCAL);
  query.setKeyBase16(encodeBase16(seed) + ':' + encodeBase16(bytes));
  return query;
};
