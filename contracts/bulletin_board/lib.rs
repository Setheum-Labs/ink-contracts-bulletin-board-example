#![cfg_attr(not(feature = "std"), no_std)]

use ink_lang as ink;

// An entrypoint to all ink! smart contracts.
// When expanded, this macro will:
// * Add local type aliases to the `mod notice_pillar`, like `Environment`,
//   `AccountId`, `Balance`, `Hash`, `Timestamp` and `BlockNumber`. All will
//   resolve to defaults `ink_env::DefaultEnvironment::<type>`
// * Adds useful #[doc] macros
// * various attributes to structs and functions
// * and many others.
// For complete breakdown, one can use _expand macro recursively_ feature in
// one's favorite IDE.
//
// It enforces various invariants of the contract, including (but not limited
// to):
// * exactly one `#[ink(storage)]`
// * at least one `#[ink(constructor)]`
// * at least one `#[ink(message)]`
// * and few other.
// More can be found [here](https://use.ink/macros-attributes/contract/).
#[ink::contract]
mod bulletin_board {

    use highlighted_posts::{
        HighlightedPostsError, HighlightedPostsRef, HIGHLIGHT_POST_SELECTOR,
    };

    use ink_env::{
        call::{build_call, Call, ExecutionInput, FromAccountId, Selector},
        DefaultEnvironment, Error as InkEnvError,
    };

    use ink_lang::{
        codegen::EmitEvent, reflect::ContractEventBase,
        utils::initialize_contract,
    };
    use ink_prelude::{format, string::String};
    use ink_storage::{
        traits::{PackedLayout, SpreadAllocate, SpreadLayout},
        Mapping,
    };
    /// Errors returned by the contract's methods.
    #[derive(Debug, PartialEq, Eq, scale::Encode, scale::Decode)]
    #[cfg_attr(feature = "std", derive(scale_info::TypeInfo))]
    pub enum BulletinBoardError {
        /// There already exists a bulletin for the calling account.
        /// Only one bulletin per account is allowed.
        BulletinAlreadyExists,
        /// Bulletin not found.
        BulletinNotFound,
        /// Value transferred was too low to pay for the listing.
        /// Transfer `listing_cost` to pay for the post.
        ListingCostTooLow(u128),
        /// Error returned when interacting with the `HighlightedPosts`
        /// contract.
        // NOTE: This is a wrapper around an error returned from calling a
        // different contract.
        HighlightError(HighlightedPostsError),
        /// An interaction with ink! environment has failed
        InkEnvError(String),
    }

    impl From<HighlightedPostsError> for BulletinBoardError {
        fn from(e: HighlightedPostsError) -> Self {
            BulletinBoardError::HighlightError(e)
        }
    }

    impl From<InkEnvError> for BulletinBoardError {
        fn from(e: InkEnvError) -> Self {
            BulletinBoardError::InkEnvError(format!("{:?}", e))
        }
    }

    // Alias for wrapper around all events in this contract generated by ink!.
    type Event = <BulletinBoard as ContractEventBase>::Type;

    // We will be storing `Bulletin` as _value_ in our map and for that reason
    // it has to implement `PackedLayout` -- see type boundaries on
    // `map.insert`.
    // `SpreadLayout` is required by `PackedLayout` -- so we have to add it
    // otherwise rustc complains.
    // `scale::Encode` and `scale::Decode` are required by the implementation of
    // `PackedLayout`.
    #[derive(
        Debug,
        PartialEq,
        Eq,
        scale::Encode,
        scale::Decode,
        SpreadLayout,
        PackedLayout,
    )]
    #[cfg_attr(feature = "std", derive(scale_info::TypeInfo))]
    pub struct Bulletin {
        author: AccountId,
        posted_at: BlockNumber,
        expires_at: BlockNumber,
        text: String,
    }

    /// Defines the storage of your contract.
    //
    // There must be exactly one `#[ink(storage)]` struct.
    #[ink(storage)]
    // Concerete `*Allocate` (either `SpreadAllocate` or `PackedAllocate`)
    // implementations will define how should your data be stored in the
    // blockchain database:
    // * whether separate fields should be stored together or separately
    // * how to chunk the data
    // * whether to pad it when it doesn't fill in full 256bytes of the cell
    // * etc.
    // For more information consult ink! documentation.
    #[derive(SpreadAllocate)]
    pub struct BulletinBoard {
        // Monotonically increasing counter for assigning unique IDs to
        // bulletins.
        id_counter: u32,
        // Store a mapping from AccountIds to a bulleting IDs.
        id_map: Mapping<AccountId, u32>,
        bulletin_map: Mapping<u32, Bulletin>,
        price_per_block_listing: u128,
        elements_count: u32,
        // Optional address of the contract we will call to highlight posts.
        highlighted_posts_board: Option<AccountId>,
    }

    impl BulletinBoard {
        // Doc string are added to the `metadata.json` file generated when
        // contract is built. Documentation can be then consumed by other
        // clients (UI clients). For more information see the [documentation](https://use.ink/metadata).
        //
        /// Creates an instance of the bulletin board contract.
        /// The `price_per_block_listing` specifies the price of listing the
        /// post for every block.
        #[ink(constructor)]
        pub fn new(
            price_per_block_listing: u128,
            highlighted_posts_board: AccountId,
        ) -> Self {
            // This call is required in order to correctly initialize the
            // `Mapping`s of our contract.
            initialize_contract(|instance: &mut BulletinBoard| {
                instance.id_counter = 0;
                instance.price_per_block_listing = price_per_block_listing;
                instance.elements_count = 0;
                instance.highlighted_posts_board =
                    Some(highlighted_posts_board);
            })
        }

        // Constructors can delegate to other constructors.
        #[ink(constructor)]
        pub fn free() -> Self {
            initialize_contract(|instance: &mut BulletinBoard| {
                instance.id_counter = 0;
                instance.price_per_block_listing = 0;
                instance.elements_count = 0;
                instance.highlighted_posts_board = None;
            })
        }

        /// Adds new post to the "bulletin board".
        /// Parameters of the post are:
        /// * `expires_after` - block number until which this post should be
        ///   considered valid.
        /// * `test` - text of the new post.
        /// Accepts a value (token) transfer if the post needs to be paid for.
        ///
        /// Fails if any of the following is true:
        /// * not enough tokens have been transferred to cover the cost of
        ///   posting,
        /// * there already exists a post from the caller's account.
        ///
        /// # Note
        ///
        /// The method needs to be annotated with `payable`; only then it is
        /// allowed to receive value as part of the call.
        #[ink(message, payable)]
        pub fn post(
            &mut self,
            expires_after: BlockNumber,
            text: String,
        ) -> Result<(), BulletinBoardError> {
            // There are over 20 methods available in `Self:env()` that give
            // contract access to the "blockchain context".
            let caller = Self::env().caller();

            // We can add `println!` statements that will be written to stdout
            // during the test.
            ink_env::debug_println!("`{:?}` wants to create a post that expires after `{:?}` blocks \
                with the text `{:?}`", caller, expires_after, text);

            if self.id_map.contains(&caller) {
                // Contract's methods can return `Result::Err` variant
                // so that the caller can handle it and recover. This does not
                // fail and rollback the transaction.
                return Err(BulletinBoardError::BulletinAlreadyExists);
            }

            // Check how much tokens have been transferred as part of the
            // transaction.
            let transferred_amount = self.env().transferred_value();

            let listing_cost = self
                .price_per_block_listing
                .checked_mul(expires_after as u128)
                .unwrap_or(u128::MAX);

            // Returning `Result::Err` reverts callee's state but makes it
            // simpler for the caller to handle an error - in contract to
            // `panic!` that doesn't contain any additional
            // information and is simply `CalleeTrapped`.
            if transferred_amount < listing_cost {
                return Err(BulletinBoardError::ListingCostTooLow(
                    listing_cost,
                ));
            }

            let curr_block_number = Self::env().block_number();
            let event =
                self._post(curr_block_number, expires_after, caller, text);
            self.highlight_post(event.author, event.id, listing_cost)?;
            self.reimburse(caller, transferred_amount - listing_cost);
            Self::emit_event(Self::env(), Event::BulletinPosted(event));
            Ok(())
        }

        /// Delets the post from the caller (if exists).
        #[ink(message)]
        pub fn delete(&mut self) -> Result<(), BulletinBoardError> {
            let author = Self::env().caller();
            let bulletin_id = self.delete_bulletin(author)?;
            self.delete_highlight(author)?;
            Self::emit_event(
                Self::env(),
                Event::BulletinRemoved(BulletinRemoved {
                    author,
                    id: bulletin_id,
                }),
            );
            Ok(())
        }

        /// Returns the post created by the caller.
        #[ink(message)]
        pub fn get_by_account(
            &self,
            account_id: AccountId,
        ) -> Option<Bulletin> {
            self.get_value(&account_id)
        }

        /// Returns the post referenced by the ID of the bulletin.
        #[ink(message)]
        pub fn get_by_id(&self, id: u32) -> Option<Bulletin> {
            self.bulletin_map.get(&id)
        }

        // To terminate a contract means to delete it from the blockchain
        // storage. One can choose whether to transfer the contract's
        // balance to others, for example a caller.
        /// Terminates the contract, iff the board is empty, with the caller as
        /// beneficiary.
        #[ink(message)]
        pub fn teminate_contract(&mut self) {
            if self.elements_count == 0 {
                self.env().terminate_contract(self.env().caller());
            }
        }

        /// Reimburses the caller with overpaid tokens.
        /// Panics if the transfer fails - this means this contract's balance is
        /// too low which means something went wrong.
        fn reimburse(&self, recipient: AccountId, amount: u128) {
            if Self::env().transfer(recipient, amount).is_err() {
                panic!("failed to reimburse the caller")
            }
        }

        // Private method that doesn't need to interact with the "blockchain
        // env". Useful for testing where there's a layer of a logic
        // that can't be easily mocked.
        fn insert_bulletin(
            &mut self,
            caller: &AccountId,
            bulletin: Bulletin,
        ) -> u32 {
            let bulletin_id = self.id_counter;
            self.id_map.insert(caller, &bulletin_id);
            self.bulletin_map.insert(bulletin_id, &bulletin);
            self.id_counter = bulletin_id + 1;
            bulletin_id
        }

        fn get_value(&self, caller: &AccountId) -> Option<Bulletin> {
            if let Some(bulletin_id) = self.id_map.get(caller) {
                let bulletin = self.bulletin_map.get(bulletin_id).unwrap_or_else(|| {
                    // Contracts can also panic - this WILL fail and rollback the
                    // transaction. Caller can still handle it and
                    // recover but there will be no additional information about the error available. 
                    // Use when you know something *unexpected* happened.
                    panic!(
                        "broken invariant: expected entry to exist for the caller"
                    )
                });
                Some(bulletin)
            } else {
                None
            }
        }

        fn delete_bulletin(
            &mut self,
            caller: AccountId,
        ) -> Result<u32, BulletinBoardError> {
            match self.id_map.get(caller) {
                None => return Err(BulletinBoardError::BulletinNotFound),
                Some(bulletin_id) => {
                    self.bulletin_map.remove(bulletin_id);
                    self.id_map.remove(caller);
                    self.elements_count -= 1;
                    Ok(bulletin_id)
                }
            }
        }

        fn _post(
            &mut self,
            curr_block: BlockNumber,
            expires_after: BlockNumber,
            author: AccountId,
            text: String,
        ) -> BulletinPosted {
            let expires_at = curr_block + expires_after;

            let bulletin = Bulletin {
                author,
                posted_at: curr_block,
                expires_at,
                text,
            };
            let bulletin_id = self.insert_bulletin(&author, bulletin);
            self.elements_count += 1;
            BulletinPosted {
                author,
                expires_at,
                id: bulletin_id,
            }
        }

        // Constructs the cross-contract call using the manual builder pattern.
        // It's more verbose than the `*Ref` pattern and we need to be more
        // careful to use proper types.
        fn highlight_post(
            &self,
            author: AccountId,
            id: u32,
            cost: u128,
        ) -> Result<(), BulletinBoardError> {
            if let Some(highlight_board) = self.highlighted_posts_board {
                build_call::<DefaultEnvironment>()
                    .call_type(Call::new().callee(highlight_board)) // Address of the contract we want to call
                    .exec_input(
                        ExecutionInput::new(Selector::new(
                            HIGHLIGHT_POST_SELECTOR, /* Selector tells ink!
                                                      * which method on that
                                                      * contract to call */
                        ))
                        .push_arg(author) // Pass in arguments of the method.
                        .push_arg(id),
                    )
                    .transferred_value(cost) // We can transfer tokens forward to another contract.
                    .returns::<Result<(), HighlightedPostsError>>()
                    .fire()??; // Double unwrap: first `?` deals with ink!'s
                               // native error `InkError`, while the second one
                               // deals with the result of the call - in this
                               // case it's `Result<(), HighlightedPostsError>`.
            }
            Ok(())
        }

        // Constructs the cross-contract call using the `*Ref` pattern.
        // More type-safe than the manual builder pattern.
        // NOTE: Currently it does not support transferring tokens, unlike the
        // builder pattern with `transferred_value` method.
        fn delete_highlight(
            &self,
            author: AccountId,
        ) -> Result<(), BulletinBoardError> {
            if let Some(highlight_board) = self.highlighted_posts_board {
                <HighlightedPostsRef as FromAccountId<
                    super::bulletin_board::Environment,
                >>::from_account_id(highlight_board)
                .delete_by_author(author)?
            }
            Ok(())
        }

        // As soon as another contract that emits events is introduced as
        // dependency, see `highlighted_posts` in `Cargo.toml`,
        // compilation fails as it can't resolve which event is going to be
        // returned from the contract.
        // To verify, replace calls to `Self::emit_event` with
        // `self::env().emit_event(_)`.
        fn emit_event<EE>(emitter: EE, event: Event)
        where
            EE: EmitEvent<BulletinBoard>,
        {
            emitter.emit_event(event);
        }
    }

    /// Defines an event that is emitted every time value is incremented.
    #[ink(event)]
    pub struct BulletinPosted {
        // `topic` tag allows simplifies lookup of the events.
        // If you mark field as `topic` it will be indexed.
        //
        // If you don't want event to be indexed, see [`#[ink(anonymous)]`](https://use.ink/macros-attributes/anonymous/).
        #[ink(topic)]
        author: AccountId,
        expires_at: BlockNumber,
        id: u32,
    }

    #[ink(event)]
    pub struct BulletinRemoved {
        #[ink(topic)]
        author: AccountId,
        id: u32,
    }
    /// Unit tests in Rust are normally defined within such a `#[cfg(test)]`
    /// module and test functions are marked with a `#[test]` attribute.
    /// The below code is technically just normal Rust code.
    #[cfg(test)]
    mod tests {
        /// Imports all the definitions from the outer scope so we can use
        /// them here.
        use super::*;

        use ink_lang as ink;

        use ink_env::test::{
            default_accounts, get_account_balance, recorded_events,
            DefaultAccounts, EmittedEvent,
        };
        use scale::Decode;

        // Returns accounts that are pre-seeded in the test database.
        // We can use them as authors for transactions.
        fn get_default_test_accounts(
        ) -> DefaultAccounts<ink_env::DefaultEnvironment> {
            default_accounts::<ink_env::DefaultEnvironment>()
        }

        // Returns balance of test account.
        fn get_balance(account_id: AccountId) -> Balance {
            get_account_balance::<ink_env::DefaultEnvironment>(account_id)
                .expect("Cannot get account balance")
        }

        // Sets caller returned by the next `Self::env().caller()` method call
        // in the contract.
        fn set_caller(caller: AccountId) {
            ink_env::test::set_caller::<ink_env::DefaultEnvironment>(caller);
        }

        const PRICE_PER_BLOCK_COST: u128 = 10;

        #[test]
        fn constructor_works() {
            // let instance = BulletinBoard::new(PRICE_PER_BLOCK_COST);
            // assert_eq!(instance.price_per_block_listing,
            // PRICE_PER_BLOCK_COST);

            let default = BulletinBoard::free();
            assert_eq!(default.price_per_block_listing, 0);
        }

        #[ink::test]
        fn post_free_succeeds() {
            let accounts = get_default_test_accounts();
            let alice = accounts.alice;
            let mut instance = BulletinBoard::free();
            let expire_after: BlockNumber = 100;
            let text: ink_prelude::string::String = "Text".into();
            // Setting Alice as the first caller is not strictly required as
            // it's the default for ink tests. We do it for clarity
            // though.
            set_caller(alice);
            assert!(
                instance.post(expire_after, text.clone()).is_ok(),
                "posting was expected to succeed"
            );
            let expected_bulletin = Bulletin {
                author: alice,
                posted_at: 0,
                expires_at: 100,
                text,
            };
            assert_eq!(
                instance.get_by_id(0).expect("to find bulletin"),
                expected_bulletin
            );
            assert_eq!(
                instance.get_by_account(alice).expect("to find bulletin"),
                expected_bulletin
            );
            let frank = accounts.frank;
            assert!(
                instance.get_by_account(frank).is_none(),
                "There should be no posts from Frank"
            );
        }

        #[ink::test]
        fn event_on_post() {
            let mut instance = BulletinBoard::free();
            let bulletin = post_from_alice(&mut instance);
            // We can verify that the proper events have been emitted during the
            // call.
            let recorded_events = recorded_events().collect::<Vec<_>>();
            assert_expected_post_event(
                &recorded_events[0],
                bulletin.author,
                bulletin.expires_at,
                0,
            );
        }

        fn assert_expected_post_event(
            event: &EmittedEvent,
            expected_author: AccountId,
            expires_at_block: BlockNumber,
            expected_id: u32,
        ) {
            let decoded_event = <Event as Decode>::decode(&mut &event.data[..])
                .expect("encountered invalid contract event data buffer");
            if let Event::BulletinPosted(BulletinPosted {
                author,
                expires_at,
                id,
            }) = decoded_event
            {
                assert_eq!(author, expected_author);
                assert_eq!(expires_at, expires_at_block);
                assert_eq!(id, expected_id);
            } else {
                panic!("encountered unexpected event kind: expected `BulletinPosted`")
            };
        }

        fn post_from_alice(instance: &mut BulletinBoard) -> Bulletin {
            let accounts = get_default_test_accounts();
            let alice = accounts.alice;
            let expire_after: BlockNumber = 100;
            let text: ink_prelude::string::String = "Text".into();
            let expected_bulletin = Bulletin {
                author: alice,
                posted_at: 0,
                expires_at: 100,
                text: text.clone(),
            };
            set_caller(alice);
            assert!(
                instance.post(expire_after, text).is_ok(),
                "posting was expected to succeed"
            );
            expected_bulletin
        }

        #[ink::test]
        fn delete_works() {
            let accounts = get_default_test_accounts();
            let alice = accounts.alice;
            let mut instance = BulletinBoard::free();
            assert!(
                matches!(
                    instance.delete(),
                    Result::Err(BulletinBoardError::BulletinNotFound)
                ),
                "no posts from Alice expected to be found on the board"
            );
            let alice_bulletin = post_from_alice(&mut instance);
            assert_eq!(
                instance.get_by_account(alice).unwrap(),
                alice_bulletin,
                "Bulletin was expected to be found after posting"
            );
            set_caller(accounts.frank);
            assert!(
                matches!(
                    instance.delete(),
                    Result::Err(BulletinBoardError::BulletinNotFound)
                ),
                "no posts from Frank"
            );
            set_caller(accounts.alice);
            assert!(instance.delete().is_ok(), "deletion should succeed");
            assert!(
                matches!(
                    instance.delete(),
                    Result::Err(BulletinBoardError::BulletinNotFound)
                ),
                "second deletion should return an error"
            );
            assert!(
                instance.get_by_account(alice).is_none(),
                "expected no posts after deleting"
            );
        }

        #[ink::test]
        fn event_on_delete() {
            let mut instance = BulletinBoard::free();
            let _bulletin = post_from_alice(&mut instance);
            assert!(instance.delete().is_ok(), "deletion should succeed");
            let recorded_events = recorded_events().collect::<Vec<_>>();
            // The events returned by `recored_events()` are all events emitted
            // since the beginning of the test.
            // The first event (at index 0) will be `BulletinPosted` by Alice
            // from the couple lines back.
            // The second event (at index 1) will be the `BulletinRemoved`
            // event.
            assert_bulletin_removed_event(
                &recorded_events[1],
                get_default_test_accounts().alice,
                0,
            );
        }

        fn assert_bulletin_removed_event(
            event: &EmittedEvent,
            expected_author: AccountId,
            expected_id: u32,
        ) {
            let decoded_event = <Event as Decode>::decode(&mut &event.data[..])
                .expect("encountered invalid contract event data buffer");
            if let Event::BulletinRemoved(BulletinRemoved { author, id }) =
                decoded_event
            {
                assert_eq!(author, expected_author);
                assert_eq!(id, expected_id);
            } else {
                panic!("encountered unexpected event kind: expected `BulletinRemoved`")
            }
        }

        // #[ink::test]
        // fn post_has_to_be_paid_for() {
        //     let accounts = get_default_test_accounts();
        //     let cost_per_block = 10;
        //     let mut instance = BulletinBoard::new(cost_per_block);
        //     let expires_after = 100;
        //     let expected_listing_cost = cost_per_block * expires_after;
        //     let text: ink_prelude::string::String = "Text".into();
        //     let pre_post_balance = get_balance(accounts.alice);
        //     assert!(
        //         matches!(
        //             instance.post(expires_after as BlockNumber,
        // text.clone()),
        // Result::Err(BulletinBoardError::ListingCostTooLow(listing_cost)) if
        // listing_cost == expected_listing_cost         ),
        //         "posting was expected to fail"
        //     );
        //     assert_eq!(
        //         pre_post_balance,
        //         get_balance(accounts.alice),
        //         "failed transaction shouldn't transfer"
        //     );
        //     // To emulate method invocation with value(token) transfer we use
        //     // `pay_with_call!` macro.
        //     let post_result = ink_env::pay_with_call!(
        //         instance.post(expires_after as BlockNumber, text),
        //         expected_listing_cost
        //     );
        //     assert!(post_result.is_ok());
        //     assert_eq!(
        //         get_balance(accounts.alice),
        //         pre_post_balance + expected_listing_cost,
        //         "Alice's balance should decrease by the cost listing",
        //     );
        // }
    }
}
