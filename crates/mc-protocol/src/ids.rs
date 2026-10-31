// AUTO-GENERATED przez tools/gen_packet_ids.py z reports/packets.json (Minecraft 1.21.1, protokół 767).
// Nie edytować ręcznie.
#![allow(dead_code)]

pub const PROTOCOL_VERSION: i32 = 767;

pub mod handshake {
    pub mod serverbound {
        pub const INTENTION: i32 = 0;
    }
}

pub mod status {
    pub mod serverbound {
        pub const STATUS_REQUEST: i32 = 0;
        pub const PING_REQUEST: i32 = 1;
    }
    pub mod clientbound {
        pub const STATUS_RESPONSE: i32 = 0;
        pub const PONG_RESPONSE: i32 = 1;
    }
}

pub mod login {
    pub mod serverbound {
        pub const HELLO: i32 = 0;
        pub const KEY: i32 = 1;
        pub const CUSTOM_QUERY_ANSWER: i32 = 2;
        pub const LOGIN_ACKNOWLEDGED: i32 = 3;
        pub const COOKIE_RESPONSE: i32 = 4;
    }
    pub mod clientbound {
        pub const LOGIN_DISCONNECT: i32 = 0;
        pub const HELLO: i32 = 1;
        pub const GAME_PROFILE: i32 = 2;
        pub const LOGIN_COMPRESSION: i32 = 3;
        pub const CUSTOM_QUERY: i32 = 4;
        pub const COOKIE_REQUEST: i32 = 5;
    }
}

pub mod configuration {
    pub mod serverbound {
        pub const CLIENT_INFORMATION: i32 = 0;
        pub const COOKIE_RESPONSE: i32 = 1;
        pub const CUSTOM_PAYLOAD: i32 = 2;
        pub const FINISH_CONFIGURATION: i32 = 3;
        pub const KEEP_ALIVE: i32 = 4;
        pub const PONG: i32 = 5;
        pub const RESOURCE_PACK: i32 = 6;
        pub const SELECT_KNOWN_PACKS: i32 = 7;
    }
    pub mod clientbound {
        pub const COOKIE_REQUEST: i32 = 0;
        pub const CUSTOM_PAYLOAD: i32 = 1;
        pub const DISCONNECT: i32 = 2;
        pub const FINISH_CONFIGURATION: i32 = 3;
        pub const KEEP_ALIVE: i32 = 4;
        pub const PING: i32 = 5;
        pub const RESET_CHAT: i32 = 6;
        pub const REGISTRY_DATA: i32 = 7;
        pub const RESOURCE_PACK_POP: i32 = 8;
        pub const RESOURCE_PACK_PUSH: i32 = 9;
        pub const STORE_COOKIE: i32 = 10;
        pub const TRANSFER: i32 = 11;
        pub const UPDATE_ENABLED_FEATURES: i32 = 12;
        pub const UPDATE_TAGS: i32 = 13;
        pub const SELECT_KNOWN_PACKS: i32 = 14;
        pub const CUSTOM_REPORT_DETAILS: i32 = 15;
        pub const SERVER_LINKS: i32 = 16;
    }
}

pub mod play {
    pub mod serverbound {
        pub const ACCEPT_TELEPORTATION: i32 = 0;
        pub const BLOCK_ENTITY_TAG_QUERY: i32 = 1;
        pub const CHANGE_DIFFICULTY: i32 = 2;
        pub const CHAT_ACK: i32 = 3;
        pub const CHAT_COMMAND: i32 = 4;
        pub const CHAT_COMMAND_SIGNED: i32 = 5;
        pub const CHAT: i32 = 6;
        pub const CHAT_SESSION_UPDATE: i32 = 7;
        pub const CHUNK_BATCH_RECEIVED: i32 = 8;
        pub const CLIENT_COMMAND: i32 = 9;
        pub const CLIENT_INFORMATION: i32 = 10;
        pub const COMMAND_SUGGESTION: i32 = 11;
        pub const CONFIGURATION_ACKNOWLEDGED: i32 = 12;
        pub const CONTAINER_BUTTON_CLICK: i32 = 13;
        pub const CONTAINER_CLICK: i32 = 14;
        pub const CONTAINER_CLOSE: i32 = 15;
        pub const CONTAINER_SLOT_STATE_CHANGED: i32 = 16;
        pub const COOKIE_RESPONSE: i32 = 17;
        pub const CUSTOM_PAYLOAD: i32 = 18;
        pub const DEBUG_SAMPLE_SUBSCRIPTION: i32 = 19;
        pub const EDIT_BOOK: i32 = 20;
        pub const ENTITY_TAG_QUERY: i32 = 21;
        pub const INTERACT: i32 = 22;
        pub const JIGSAW_GENERATE: i32 = 23;
        pub const KEEP_ALIVE: i32 = 24;
        pub const LOCK_DIFFICULTY: i32 = 25;
        pub const MOVE_PLAYER_POS: i32 = 26;
        pub const MOVE_PLAYER_POS_ROT: i32 = 27;
        pub const MOVE_PLAYER_ROT: i32 = 28;
        pub const MOVE_PLAYER_STATUS_ONLY: i32 = 29;
        pub const MOVE_VEHICLE: i32 = 30;
        pub const PADDLE_BOAT: i32 = 31;
        pub const PICK_ITEM: i32 = 32;
        pub const PING_REQUEST: i32 = 33;
        pub const PLACE_RECIPE: i32 = 34;
        pub const PLAYER_ABILITIES: i32 = 35;
        pub const PLAYER_ACTION: i32 = 36;
        pub const PLAYER_COMMAND: i32 = 37;
        pub const PLAYER_INPUT: i32 = 38;
        pub const PONG: i32 = 39;
        pub const RECIPE_BOOK_CHANGE_SETTINGS: i32 = 40;
        pub const RECIPE_BOOK_SEEN_RECIPE: i32 = 41;
        pub const RENAME_ITEM: i32 = 42;
        pub const RESOURCE_PACK: i32 = 43;
        pub const SEEN_ADVANCEMENTS: i32 = 44;
        pub const SELECT_TRADE: i32 = 45;
        pub const SET_BEACON: i32 = 46;
        pub const SET_CARRIED_ITEM: i32 = 47;
        pub const SET_COMMAND_BLOCK: i32 = 48;
        pub const SET_COMMAND_MINECART: i32 = 49;
        pub const SET_CREATIVE_MODE_SLOT: i32 = 50;
        pub const SET_JIGSAW_BLOCK: i32 = 51;
        pub const SET_STRUCTURE_BLOCK: i32 = 52;
        pub const SIGN_UPDATE: i32 = 53;
        pub const SWING: i32 = 54;
        pub const TELEPORT_TO_ENTITY: i32 = 55;
        pub const USE_ITEM_ON: i32 = 56;
        pub const USE_ITEM: i32 = 57;
    }
    pub mod clientbound {
        pub const BUNDLE_DELIMITER: i32 = 0;
        pub const ADD_ENTITY: i32 = 1;
        pub const ADD_EXPERIENCE_ORB: i32 = 2;
        pub const ANIMATE: i32 = 3;
        pub const AWARD_STATS: i32 = 4;
        pub const BLOCK_CHANGED_ACK: i32 = 5;
        pub const BLOCK_DESTRUCTION: i32 = 6;
        pub const BLOCK_ENTITY_DATA: i32 = 7;
        pub const BLOCK_EVENT: i32 = 8;
        pub const BLOCK_UPDATE: i32 = 9;
        pub const BOSS_EVENT: i32 = 10;
        pub const CHANGE_DIFFICULTY: i32 = 11;
        pub const CHUNK_BATCH_FINISHED: i32 = 12;
        pub const CHUNK_BATCH_START: i32 = 13;
        pub const CHUNKS_BIOMES: i32 = 14;
        pub const CLEAR_TITLES: i32 = 15;
        pub const COMMAND_SUGGESTIONS: i32 = 16;
        pub const COMMANDS: i32 = 17;
        pub const CONTAINER_CLOSE: i32 = 18;
        pub const CONTAINER_SET_CONTENT: i32 = 19;
        pub const CONTAINER_SET_DATA: i32 = 20;
        pub const CONTAINER_SET_SLOT: i32 = 21;
        pub const COOKIE_REQUEST: i32 = 22;
        pub const COOLDOWN: i32 = 23;
        pub const CUSTOM_CHAT_COMPLETIONS: i32 = 24;
        pub const CUSTOM_PAYLOAD: i32 = 25;
        pub const DAMAGE_EVENT: i32 = 26;
        pub const DEBUG_SAMPLE: i32 = 27;
        pub const DELETE_CHAT: i32 = 28;
        pub const DISCONNECT: i32 = 29;
        pub const DISGUISED_CHAT: i32 = 30;
        pub const ENTITY_EVENT: i32 = 31;
        pub const EXPLODE: i32 = 32;
        pub const FORGET_LEVEL_CHUNK: i32 = 33;
        pub const GAME_EVENT: i32 = 34;
        pub const HORSE_SCREEN_OPEN: i32 = 35;
        pub const HURT_ANIMATION: i32 = 36;
        pub const INITIALIZE_BORDER: i32 = 37;
        pub const KEEP_ALIVE: i32 = 38;
        pub const LEVEL_CHUNK_WITH_LIGHT: i32 = 39;
        pub const LEVEL_EVENT: i32 = 40;
        pub const LEVEL_PARTICLES: i32 = 41;
        pub const LIGHT_UPDATE: i32 = 42;
        pub const LOGIN: i32 = 43;
        pub const MAP_ITEM_DATA: i32 = 44;
        pub const MERCHANT_OFFERS: i32 = 45;
        pub const MOVE_ENTITY_POS: i32 = 46;
        pub const MOVE_ENTITY_POS_ROT: i32 = 47;
        pub const MOVE_ENTITY_ROT: i32 = 48;
        pub const MOVE_VEHICLE: i32 = 49;
        pub const OPEN_BOOK: i32 = 50;
        pub const OPEN_SCREEN: i32 = 51;
        pub const OPEN_SIGN_EDITOR: i32 = 52;
        pub const PING: i32 = 53;
        pub const PONG_RESPONSE: i32 = 54;
        pub const PLACE_GHOST_RECIPE: i32 = 55;
        pub const PLAYER_ABILITIES: i32 = 56;
        pub const PLAYER_CHAT: i32 = 57;
        pub const PLAYER_COMBAT_END: i32 = 58;
        pub const PLAYER_COMBAT_ENTER: i32 = 59;
        pub const PLAYER_COMBAT_KILL: i32 = 60;
        pub const PLAYER_INFO_REMOVE: i32 = 61;
        pub const PLAYER_INFO_UPDATE: i32 = 62;
        pub const PLAYER_LOOK_AT: i32 = 63;
        pub const PLAYER_POSITION: i32 = 64;
        pub const RECIPE: i32 = 65;
        pub const REMOVE_ENTITIES: i32 = 66;
        pub const REMOVE_MOB_EFFECT: i32 = 67;
        pub const RESET_SCORE: i32 = 68;
        pub const RESOURCE_PACK_POP: i32 = 69;
        pub const RESOURCE_PACK_PUSH: i32 = 70;
        pub const RESPAWN: i32 = 71;
        pub const ROTATE_HEAD: i32 = 72;
        pub const SECTION_BLOCKS_UPDATE: i32 = 73;
        pub const SELECT_ADVANCEMENTS_TAB: i32 = 74;
        pub const SERVER_DATA: i32 = 75;
        pub const SET_ACTION_BAR_TEXT: i32 = 76;
        pub const SET_BORDER_CENTER: i32 = 77;
        pub const SET_BORDER_LERP_SIZE: i32 = 78;
        pub const SET_BORDER_SIZE: i32 = 79;
        pub const SET_BORDER_WARNING_DELAY: i32 = 80;
        pub const SET_BORDER_WARNING_DISTANCE: i32 = 81;
        pub const SET_CAMERA: i32 = 82;
        pub const SET_CARRIED_ITEM: i32 = 83;
        pub const SET_CHUNK_CACHE_CENTER: i32 = 84;
        pub const SET_CHUNK_CACHE_RADIUS: i32 = 85;
        pub const SET_DEFAULT_SPAWN_POSITION: i32 = 86;
        pub const SET_DISPLAY_OBJECTIVE: i32 = 87;
        pub const SET_ENTITY_DATA: i32 = 88;
        pub const SET_ENTITY_LINK: i32 = 89;
        pub const SET_ENTITY_MOTION: i32 = 90;
        pub const SET_EQUIPMENT: i32 = 91;
        pub const SET_EXPERIENCE: i32 = 92;
        pub const SET_HEALTH: i32 = 93;
        pub const SET_OBJECTIVE: i32 = 94;
        pub const SET_PASSENGERS: i32 = 95;
        pub const SET_PLAYER_TEAM: i32 = 96;
        pub const SET_SCORE: i32 = 97;
        pub const SET_SIMULATION_DISTANCE: i32 = 98;
        pub const SET_SUBTITLE_TEXT: i32 = 99;
        pub const SET_TIME: i32 = 100;
        pub const SET_TITLE_TEXT: i32 = 101;
        pub const SET_TITLES_ANIMATION: i32 = 102;
        pub const SOUND_ENTITY: i32 = 103;
        pub const SOUND: i32 = 104;
        pub const START_CONFIGURATION: i32 = 105;
        pub const STOP_SOUND: i32 = 106;
        pub const STORE_COOKIE: i32 = 107;
        pub const SYSTEM_CHAT: i32 = 108;
        pub const TAB_LIST: i32 = 109;
        pub const TAG_QUERY: i32 = 110;
        pub const TAKE_ITEM_ENTITY: i32 = 111;
        pub const TELEPORT_ENTITY: i32 = 112;
        pub const TICKING_STATE: i32 = 113;
        pub const TICKING_STEP: i32 = 114;
        pub const TRANSFER: i32 = 115;
        pub const UPDATE_ADVANCEMENTS: i32 = 116;
        pub const UPDATE_ATTRIBUTES: i32 = 117;
        pub const UPDATE_MOB_EFFECT: i32 = 118;
        pub const UPDATE_RECIPES: i32 = 119;
        pub const UPDATE_TAGS: i32 = 120;
        pub const PROJECTILE_POWER: i32 = 121;
        pub const CUSTOM_REPORT_DETAILS: i32 = 122;
        pub const SERVER_LINKS: i32 = 123;
    }
}

pub mod names {
    pub fn handshake_serverbound(id: i32) -> &'static str {
        match id {
            0 => "intention",
            _ => "?",
        }
    }
    pub fn status_serverbound(id: i32) -> &'static str {
        match id {
            0 => "status_request",
            1 => "ping_request",
            _ => "?",
        }
    }
    pub fn status_clientbound(id: i32) -> &'static str {
        match id {
            0 => "status_response",
            1 => "pong_response",
            _ => "?",
        }
    }
    pub fn login_serverbound(id: i32) -> &'static str {
        match id {
            0 => "hello",
            1 => "key",
            2 => "custom_query_answer",
            3 => "login_acknowledged",
            4 => "cookie_response",
            _ => "?",
        }
    }
    pub fn login_clientbound(id: i32) -> &'static str {
        match id {
            0 => "login_disconnect",
            1 => "hello",
            2 => "game_profile",
            3 => "login_compression",
            4 => "custom_query",
            5 => "cookie_request",
            _ => "?",
        }
    }
    pub fn configuration_serverbound(id: i32) -> &'static str {
        match id {
            0 => "client_information",
            1 => "cookie_response",
            2 => "custom_payload",
            3 => "finish_configuration",
            4 => "keep_alive",
            5 => "pong",
            6 => "resource_pack",
            7 => "select_known_packs",
            _ => "?",
        }
    }
    pub fn configuration_clientbound(id: i32) -> &'static str {
        match id {
            0 => "cookie_request",
            1 => "custom_payload",
            2 => "disconnect",
            3 => "finish_configuration",
            4 => "keep_alive",
            5 => "ping",
            6 => "reset_chat",
            7 => "registry_data",
            8 => "resource_pack_pop",
            9 => "resource_pack_push",
            10 => "store_cookie",
            11 => "transfer",
            12 => "update_enabled_features",
            13 => "update_tags",
            14 => "select_known_packs",
            15 => "custom_report_details",
            16 => "server_links",
            _ => "?",
        }
    }
    pub fn play_serverbound(id: i32) -> &'static str {
        match id {
            0 => "accept_teleportation",
            1 => "block_entity_tag_query",
            2 => "change_difficulty",
            3 => "chat_ack",
            4 => "chat_command",
            5 => "chat_command_signed",
            6 => "chat",
            7 => "chat_session_update",
            8 => "chunk_batch_received",
            9 => "client_command",
            10 => "client_information",
            11 => "command_suggestion",
            12 => "configuration_acknowledged",
            13 => "container_button_click",
            14 => "container_click",
            15 => "container_close",
            16 => "container_slot_state_changed",
            17 => "cookie_response",
            18 => "custom_payload",
            19 => "debug_sample_subscription",
            20 => "edit_book",
            21 => "entity_tag_query",
            22 => "interact",
            23 => "jigsaw_generate",
            24 => "keep_alive",
            25 => "lock_difficulty",
            26 => "move_player_pos",
            27 => "move_player_pos_rot",
            28 => "move_player_rot",
            29 => "move_player_status_only",
            30 => "move_vehicle",
            31 => "paddle_boat",
            32 => "pick_item",
            33 => "ping_request",
            34 => "place_recipe",
            35 => "player_abilities",
            36 => "player_action",
            37 => "player_command",
            38 => "player_input",
            39 => "pong",
            40 => "recipe_book_change_settings",
            41 => "recipe_book_seen_recipe",
            42 => "rename_item",
            43 => "resource_pack",
            44 => "seen_advancements",
            45 => "select_trade",
            46 => "set_beacon",
            47 => "set_carried_item",
            48 => "set_command_block",
            49 => "set_command_minecart",
            50 => "set_creative_mode_slot",
            51 => "set_jigsaw_block",
            52 => "set_structure_block",
            53 => "sign_update",
            54 => "swing",
            55 => "teleport_to_entity",
            56 => "use_item_on",
            57 => "use_item",
            _ => "?",
        }
    }
    pub fn play_clientbound(id: i32) -> &'static str {
        match id {
            0 => "bundle_delimiter",
            1 => "add_entity",
            2 => "add_experience_orb",
            3 => "animate",
            4 => "award_stats",
            5 => "block_changed_ack",
            6 => "block_destruction",
            7 => "block_entity_data",
            8 => "block_event",
            9 => "block_update",
            10 => "boss_event",
            11 => "change_difficulty",
            12 => "chunk_batch_finished",
            13 => "chunk_batch_start",
            14 => "chunks_biomes",
            15 => "clear_titles",
            16 => "command_suggestions",
            17 => "commands",
            18 => "container_close",
            19 => "container_set_content",
            20 => "container_set_data",
            21 => "container_set_slot",
            22 => "cookie_request",
            23 => "cooldown",
            24 => "custom_chat_completions",
            25 => "custom_payload",
            26 => "damage_event",
            27 => "debug_sample",
            28 => "delete_chat",
            29 => "disconnect",
            30 => "disguised_chat",
            31 => "entity_event",
            32 => "explode",
            33 => "forget_level_chunk",
            34 => "game_event",
            35 => "horse_screen_open",
            36 => "hurt_animation",
            37 => "initialize_border",
            38 => "keep_alive",
            39 => "level_chunk_with_light",
            40 => "level_event",
            41 => "level_particles",
            42 => "light_update",
            43 => "login",
            44 => "map_item_data",
            45 => "merchant_offers",
            46 => "move_entity_pos",
            47 => "move_entity_pos_rot",
            48 => "move_entity_rot",
            49 => "move_vehicle",
            50 => "open_book",
            51 => "open_screen",
            52 => "open_sign_editor",
            53 => "ping",
            54 => "pong_response",
            55 => "place_ghost_recipe",
            56 => "player_abilities",
            57 => "player_chat",
            58 => "player_combat_end",
            59 => "player_combat_enter",
            60 => "player_combat_kill",
            61 => "player_info_remove",
            62 => "player_info_update",
            63 => "player_look_at",
            64 => "player_position",
            65 => "recipe",
            66 => "remove_entities",
            67 => "remove_mob_effect",
            68 => "reset_score",
            69 => "resource_pack_pop",
            70 => "resource_pack_push",
            71 => "respawn",
            72 => "rotate_head",
            73 => "section_blocks_update",
            74 => "select_advancements_tab",
            75 => "server_data",
            76 => "set_action_bar_text",
            77 => "set_border_center",
            78 => "set_border_lerp_size",
            79 => "set_border_size",
            80 => "set_border_warning_delay",
            81 => "set_border_warning_distance",
            82 => "set_camera",
            83 => "set_carried_item",
            84 => "set_chunk_cache_center",
            85 => "set_chunk_cache_radius",
            86 => "set_default_spawn_position",
            87 => "set_display_objective",
            88 => "set_entity_data",
            89 => "set_entity_link",
            90 => "set_entity_motion",
            91 => "set_equipment",
            92 => "set_experience",
            93 => "set_health",
            94 => "set_objective",
            95 => "set_passengers",
            96 => "set_player_team",
            97 => "set_score",
            98 => "set_simulation_distance",
            99 => "set_subtitle_text",
            100 => "set_time",
            101 => "set_title_text",
            102 => "set_titles_animation",
            103 => "sound_entity",
            104 => "sound",
            105 => "start_configuration",
            106 => "stop_sound",
            107 => "store_cookie",
            108 => "system_chat",
            109 => "tab_list",
            110 => "tag_query",
            111 => "take_item_entity",
            112 => "teleport_entity",
            113 => "ticking_state",
            114 => "ticking_step",
            115 => "transfer",
            116 => "update_advancements",
            117 => "update_attributes",
            118 => "update_mob_effect",
            119 => "update_recipes",
            120 => "update_tags",
            121 => "projectile_power",
            122 => "custom_report_details",
            123 => "server_links",
            _ => "?",
        }
    }
}
