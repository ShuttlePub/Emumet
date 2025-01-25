use crate::service::Direction;
use crate::transfer::account::AccountDto;
use kernel::interfaces::query::DependOnAccountQuery;
use kernel::KernelError;
use std::future::Future;

pub trait GetAccountService: 'static + Sync + Send + DependOnAccountQuery {
    fn get_all_accounts(
        &self,
        limit: Option<i32>,
        cursor: Option<String>,
        direction: Option<Direction>, //TODO error_stackやめてResponseに載せれる情報を返す
    ) -> impl Future<Output = error_stack::Result<Vec<AccountDto>, KernelError>> {
        async {
            todo!("get auth id")
            // self.account_query().find_by_auth_id(auth_id).await
        }
    }
}

impl<T> GetAccountService for T where T: DependOnAccountQuery + 'static {}
