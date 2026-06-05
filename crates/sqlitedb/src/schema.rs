// Copyright Alexandre D. Díaz
// Manually maintained – do NOT regenerate with `diesel print-schema` without reviewing.
// SQLite INTEGER PRIMARY KEY = i64; all FKs are also i64.

diesel::table! {
    author (id) {
        id -> BigInt,
        name -> Text,
    }
}

diesel::table! {
    committer (id) {
        id -> BigInt,
        name -> Text,
    }
}

diesel::table! {
    dependency (id) {
        id -> BigInt,
        dependency_type_id -> BigInt,
        name -> Text,
    }
}

diesel::table! {
    dependency_module (id) {
        id -> BigInt,
        dependency_id -> BigInt,
        module_id -> BigInt,
    }
}

diesel::table! {
    dependency_osv (id) {
        id -> BigInt,
        dependency_module_id -> BigInt,
        osv_id -> Text,
        details -> Text,
        fixed_in -> Text,
    }
}

diesel::table! {
    dependency_type (id) {
        id -> BigInt,
        name -> Text,
    }
}

diesel::table! {
    gh_organization (id) {
        id -> BigInt,
        name -> Text,
    }
}

diesel::table! {
    gh_repository (id) {
        id -> BigInt,
        name -> Text,
        gh_organization_id -> BigInt,
        create_date -> Text,
        update_date -> Text,
    }
}

diesel::table! {
    maintainer (id) {
        id -> BigInt,
        name -> Text,
    }
}

diesel::table! {
    module (id) {
        id -> BigInt,
        technical_name -> Text,
        version_odoo -> Integer,
        name -> Text,
        version_module -> Text,
        description -> Nullable<Text>,
        website -> Nullable<Text>,
        license -> Nullable<Text>,
        category -> Nullable<Text>,
        auto_install -> Bool,
        application -> Bool,
        installable -> Bool,
        gh_repository_id -> BigInt,
        create_date -> Text,
        update_date -> Text,
        folder_size -> BigInt,
        last_commit_hash -> Text,
        last_commit_author -> Text,
        last_commit_name -> Text,
        last_commit_date -> Text,
        last_commit_partof -> Nullable<Text>,
    }
}

diesel::table! {
    module_author (id) {
        id -> BigInt,
        module_id -> BigInt,
        author_id -> BigInt,
    }
}

diesel::table! {
    module_committer (id) {
        id -> BigInt,
        module_id -> BigInt,
        committer_id -> BigInt,
        commits -> Integer,
    }
}

diesel::table! {
    module_maintainer (id) {
        id -> BigInt,
        module_id -> BigInt,
        maintainer_id -> BigInt,
    }
}

diesel::table! {
    pull_request (id) {
        id -> BigInt,
        name -> Text,
        version_odoo -> Integer,
        module_technical_name -> Text,
        prid -> BigInt,
        gh_repository_id -> BigInt,
    }
}

diesel::table! {
    system_event (id) {
        id -> BigInt,
        message -> Text,
        date -> Text,
        event_type_id -> BigInt,
    }
}

diesel::table! {
    system_event_type (id) {
        id -> BigInt,
        name -> Text,
    }
}

diesel::allow_tables_to_appear_in_same_query!(
    author,
    committer,
    dependency,
    dependency_module,
    dependency_osv,
    dependency_type,
    gh_organization,
    gh_repository,
    maintainer,
    module,
    module_author,
    module_committer,
    module_maintainer,
    pull_request,
    system_event,
    system_event_type,
);
