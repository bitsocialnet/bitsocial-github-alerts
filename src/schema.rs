// @generated automatically by Diesel CLI.

diesel::table! {
    chats (id) {
        id -> Int4,
        #[max_length = 255]
        name -> Varchar,
        #[max_length = 255]
        telegram_id -> Varchar,
        #[max_length = 255]
        webhook_url -> Nullable<Varchar>,
        #[max_length = 255]
        thread_id -> Nullable<Varchar>,
        #[max_length = 5]
        language -> Varchar,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
        is_active -> Bool,
        deactivated_at -> Nullable<Timestamptz>,
    }
}

diesel::table! {
    webhooks (id) {
        id -> Int4,
        #[max_length = 255]
        name -> Varchar,
        #[max_length = 255]
        webhook_url -> Varchar,
        created_at -> Timestamp,
        updated_at -> Timestamp,
        chat_id -> Nullable<Int4>,
    }
}

diesel::joinable!(webhooks -> chats (chat_id));

diesel::allow_tables_to_appear_in_same_query!(chats, webhooks,);
