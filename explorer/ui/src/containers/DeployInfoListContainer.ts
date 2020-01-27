import { action, observable } from 'mobx';

import ErrorContainer from './ErrorContainer';
import { ByteArray, CasperService } from 'casperlabs-sdk';
import {
  BlockInfo,
  DeployInfo
} from 'casperlabs-grpc/io/casperlabs/casper/consensus/info_pb';

export class DeployInfoListContainer {
  @observable deployInfosList: DeployInfo[] | null = null;
  @observable pageToken: string | null = null;
  @observable nextPageToken: string | null = null;
  @observable prevPageToken: string | null = null;
  @observable accountPublicKey: ByteArray | null = null;
  pageSize: number = 5;

  constructor(
    private errors: ErrorContainer,
    private casperService: CasperService
  ) {}

  /** Call whenever the page switches to a new account. */
  @action
  init(accountPublicKey: ByteArray, pageToken: string | null) {
    this.accountPublicKey = accountPublicKey;
    this.pageToken = pageToken;
    this.deployInfosList = null;
  }

  @action
  async fetchPage(pageToken: string | null) {
    this.pageToken = pageToken;
    this.fetchData();
  }

  @action
  async fetchData() {
    if (this.accountPublicKey === null) return;
    if (this.pageToken === '') return; // no more data
    await this.errors.capture(
      this.casperService
        .getDeployInfos(
          this.accountPublicKey,
          this.pageSize,
          BlockInfo.View.BASIC,
          this.pageToken || ''
        )
        .then(listDeployInfosResponse => {
          this.deployInfosList = listDeployInfosResponse.getDeployInfosList();
          this.nextPageToken = listDeployInfosResponse.getNextPageToken();
          this.prevPageToken = listDeployInfosResponse.getPrevPageToken();
        })
    );
  }
}
