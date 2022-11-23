import { ContractPromise } from '@polkadot/api-contract';
import { displayErrorToast } from 'components/NotificationToast';
import { ErrorToastMessages, GAS_LIMIT_VALUE } from 'shared/constants';
import bulletinBoardMetadata from '../metadata/metadata_bulletin_board.json';
import addresses from '../metadata/addresses.json';
import { ApiPromise } from '@polkadot/api';

export type PostByAccount = {
  author: string;
  postedAt: number;
  expiresAt: number;
  text: string;
};

export const getPostByAccount = async (
  accountId: string,
  api: ApiPromise
): Promise<PostByAccount | null> => {
  const contract = new ContractPromise(api, bulletinBoardMetadata, addresses.bulletin_board);
  const { result, output } = await contract.query.getByAccount(
    contract.address,
    {
      gasLimit: GAS_LIMIT_VALUE,
    },
    accountId
  );
  if (result.isOk && output) {
    console.log(output.toHuman());
    return output.toHuman() as PostByAccount;
  }
  if (result.isErr) {
    displayErrorToast(ErrorToastMessages.ERROR_FETCHING_DATA);
  }
  return null;
};
