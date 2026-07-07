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
        installation -> Nullable<Text>,
        usage -> Nullable<Text>,
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
        insertions -> Integer,
        deletions -> Integer,
    }
}

diesel::table! {
    module_committer_period (id) {
        id -> BigInt,
        module_id -> BigInt,
        committer_id -> BigInt,
        year -> Integer,
        month -> Integer,
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
    module_model (id) {
        id -> BigInt,
        module_id -> BigInt,
        model_name -> Text,
        class_name -> Text,
        inherit_from -> Nullable<Text>,
        is_new_model -> Bool,
        docstring -> Nullable<Text>,
        attrs -> Nullable<Text>,
        module_version_id -> BigInt,
    }
}

diesel::table! {
    module_model_field (id) {
        id -> BigInt,
        module_model_id -> BigInt,
        name -> Text,
        field_type -> Text,
        relation -> Nullable<Text>,
        attrs -> Nullable<Text>,
    }
}

diesel::table! {
    module_model_method (id) {
        id -> BigInt,
        module_model_id -> BigInt,
        name -> Text,
        decorators -> Nullable<Text>,
        signature -> Text,
        docstring -> Nullable<Text>,
    }
}

diesel::table! {
    module_version (id) {
        id -> BigInt,
        module_id -> BigInt,
        version_module -> Text,
        create_date -> Text,
        update_date -> Text,
    }
}

diesel::table! {
    module_view (id) {
        id -> BigInt,
        module_id -> BigInt,
        xml_id -> Text,
        name -> Nullable<Text>,
        model -> Nullable<Text>,
        inherit_xml_id -> Nullable<Text>,
        view_type -> Nullable<Text>,
        module_version_id -> BigInt,
    }
}

diesel::table! {
    module_record (id) {
        id -> BigInt,
        module_id -> BigInt,
        xml_id -> Text,
        model -> Text,
        noupdate -> Bool,
        fields -> Nullable<Text>,
        module_version_id -> BigInt,
    }
}

diesel::table! {
    module_controller (id) {
        id -> BigInt,
        module_id -> BigInt,
        class_name -> Text,
        name -> Text,
        routes -> Text,
        auth -> Nullable<Text>,
        http_type -> Text,
        methods -> Nullable<Text>,
        csrf -> Nullable<Bool>,
        website -> Bool,
        uses_sudo -> Bool,
        signature -> Text,
        docstring -> Nullable<Text>,
        module_version_id -> BigInt,
    }
}

diesel::table! {
    module_security_warning (id) {
        id -> BigInt,
        module_id -> BigInt,
        severity -> Text,
        code -> Text,
        message -> Text,
        xml_id -> Nullable<Text>,
        module_version_id -> BigInt,
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
        severity -> Text,
        is_html -> Bool,
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
    module_committer_period,
    module_controller,
    module_maintainer,
    module_model,
    module_model_field,
    module_model_method,
    module_record,
    module_security_warning,
    module_version,
    module_view,
    pull_request,
    system_event,
    system_event_type,
);
