// The fake library the mock server serves.
//
// One user, one movie library, one movie whose only media source is the clip
// encoded by setup.mjs. Ids are 32-hex like Jellyfin's own GUID-without-dashes
// form, because jellyfin-web normalises and compares them as strings.

export const SERVER_ID = 'e2e00000000000000000000000000001';
export const USER_ID = 'e2e00000000000000000000000000002';
export const LIBRARY_ID = 'e2e00000000000000000000000000003';
export const ITEM_ID = 'e2e00000000000000000000000000004';
export const MEDIA_SOURCE_ID = ITEM_ID;
export const SESSION_ID = 'e2e00000000000000000000000000005';
export const ACCESS_TOKEN = 'e2e00000000000000000000000000006';

export const SERVER_NAME = 'Astrofin E2E Mock';
export const USERNAME = 'e2e';
export const PASSWORD = 'e2e-password';

/** Runtime of the generated clip, in Jellyfin ticks (100 ns). */
export const CLIP_SECONDS = 30;
export const RUNTIME_TICKS = CLIP_SECONDS * 10_000_000;

export const ITEM_NAME = 'Astrofin Smoke Clip';

export function publicSystemInfo() {
    return {
        LocalAddress: null,
        ServerName: SERVER_NAME,
        Version: '10.11.11',
        ProductName: 'Jellyfin Server',
        OperatingSystem: 'Windows',
        Id: SERVER_ID,
        StartupWizardCompleted: true,
    };
}

export function systemInfo() {
    return {
        ...publicSystemInfo(),
        HasPendingRestart: false,
        IsShuttingDown: false,
        SupportsLibraryMonitor: true,
        WebSocketPortNumber: 8096,
        CompletedInstallations: [],
        CanSelfRestart: false,
        CanLaunchWebBrowser: false,
        ProgramDataPath: 'C:/e2e/data',
        WebPath: 'C:/e2e/web',
        ItemsByNamePath: 'C:/e2e/data/metadata',
        CachePath: 'C:/e2e/cache',
        LogPath: 'C:/e2e/log',
        InternalMetadataPath: 'C:/e2e/data/metadata',
        TranscodingTempPath: 'C:/e2e/cache/transcodes',
        SystemArchitecture: 'X64',
    };
}

export function user() {
    return {
        Name: USERNAME,
        ServerId: SERVER_ID,
        Id: USER_ID,
        HasPassword: true,
        HasConfiguredPassword: true,
        HasConfiguredEasyPassword: false,
        EnableAutoLogin: false,
        LastLoginDate: new Date().toISOString(),
        LastActivityDate: new Date().toISOString(),
        Configuration: {
            AudioLanguagePreference: null,
            PlayDefaultAudioTrack: true,
            SubtitleLanguagePreference: '',
            DisplayMissingEpisodes: false,
            GroupedFolders: [],
            SubtitleMode: 'Default',
            DisplayCollectionsView: false,
            EnableLocalPassword: false,
            OrderedViews: [],
            LatestItemsExcludes: [],
            MyMediaExcludes: [],
            HidePlayedInLatest: true,
            RememberAudioSelections: true,
            RememberSubtitleSelections: true,
            EnableNextEpisodeAutoPlay: true,
            CastReceiverId: 'F007D354',
        },
        Policy: {
            IsAdministrator: true,
            IsHidden: false,
            IsDisabled: false,
            BlockedTags: [],
            EnableUserPreferenceAccess: true,
            AccessSchedules: [],
            BlockUnratedItems: [],
            EnableRemoteControlOfOtherUsers: true,
            EnableSharedDeviceControl: true,
            EnableRemoteAccess: true,
            EnableLiveTvManagement: true,
            EnableLiveTvAccess: true,
            EnableMediaPlayback: true,
            EnableAudioPlaybackTranscoding: true,
            EnableVideoPlaybackTranscoding: true,
            EnablePlaybackRemuxing: true,
            ForceRemoteSourceTranscoding: false,
            EnableContentDeletion: false,
            EnableContentDeletionFromFolders: [],
            EnableContentDownloading: true,
            EnableSyncTranscoding: true,
            EnableMediaConversion: true,
            EnabledDevices: [],
            EnableAllDevices: true,
            EnabledChannels: [],
            EnableAllChannels: true,
            EnabledFolders: [],
            EnableAllFolders: true,
            InvalidLoginAttemptCount: 0,
            LoginAttemptsBeforeLockout: -1,
            MaxActiveSessions: 0,
            EnablePublicSharing: true,
            BlockedMediaFolders: [],
            BlockedChannels: [],
            RemoteClientBitrateLimit: 0,
            AuthenticationProviderId:
                'Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider',
            PasswordResetProviderId:
                'Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider',
            SyncPlayAccess: 'CreateAndJoinGroups',
        },
    };
}

export function userData(overrides = {}) {
    return {
        PlaybackPositionTicks: 0,
        PlayCount: 0,
        IsFavorite: false,
        Played: false,
        Key: ITEM_ID,
        ItemId: ITEM_ID,
        ...overrides,
    };
}

export function library() {
    return {
        Name: 'E2E Movies',
        ServerId: SERVER_ID,
        Id: LIBRARY_ID,
        Etag: 'e2elibrary',
        DateCreated: '2026-01-01T00:00:00.0000000Z',
        CanDelete: false,
        CanDownload: false,
        SortName: 'e2e movies',
        ExternalUrls: [],
        Taglines: [],
        Genres: [],
        PlayAccess: 'Full',
        RemoteTrailers: [],
        ProviderIds: {},
        IsFolder: true,
        ParentId: 'e2e00000000000000000000000000009',
        Type: 'CollectionFolder',
        People: [],
        Studios: [],
        GenreItems: [],
        LocalTrailerCount: 0,
        UserData: { PlaybackPositionTicks: 0, PlayCount: 0, IsFavorite: false, Played: false, Key: LIBRARY_ID },
        ChildCount: 1,
        DisplayPreferencesId: LIBRARY_ID,
        Tags: [],
        PrimaryImageAspectRatio: 1,
        CollectionType: 'movies',
        ImageTags: { Primary: 'e2elibraryprimary' },
        BackdropImageTags: [],
        ImageBlurHashes: {},
        LocationType: 'FileSystem',
        MediaType: 'Unknown',
        LockedFields: [],
        LockData: false,
    };
}

/** The one video stream layout the item advertises. */
function mediaStreams() {
    return [
        {
            Codec: 'h264',
            TimeBase: '1/12288',
            VideoRange: 'SDR',
            VideoRangeType: 'SDR',
            AudioSpatialFormat: 'None',
            DisplayTitle: '360p H264 SDR',
            NalLengthSize: '4',
            IsInterlaced: false,
            IsAVC: true,
            BitRate: 1_100_000,
            BitDepth: 8,
            RefFrames: 1,
            IsDefault: true,
            IsForced: false,
            IsHearingImpaired: false,
            Height: 360,
            Width: 640,
            AverageFrameRate: 24,
            RealFrameRate: 24,
            ReferenceFrameRate: 24,
            Profile: 'Main',
            Type: 'Video',
            AspectRatio: '16:9',
            Index: 0,
            IsExternal: false,
            IsTextSubtitleStream: false,
            SupportsExternalStream: false,
            PixelFormat: 'yuv420p',
            Level: 30,
            IsAnamorphic: false,
        },
        {
            Codec: 'aac',
            CodecTag: 'mp4a',
            TimeBase: '1/48000',
            AudioSpatialFormat: 'None',
            DisplayTitle: 'AAC - Mono - Default',
            IsInterlaced: false,
            ChannelLayout: 'mono',
            BitRate: 69_000,
            Channels: 1,
            SampleRate: 48_000,
            IsDefault: true,
            IsForced: false,
            IsHearingImpaired: false,
            Profile: 'LC',
            Type: 'Audio',
            Index: 1,
            IsExternal: false,
            IsTextSubtitleStream: false,
            SupportsExternalStream: false,
            Level: 0,
        },
    ];
}

export function mediaSource({ size = 725_726, directPlay = true } = {}) {
    return {
        Protocol: 'File',
        Id: MEDIA_SOURCE_ID,
        Path: 'C:/e2e/media/clip.mp4',
        Type: 'Default',
        Container: 'mp4',
        Size: size,
        Name: ITEM_NAME,
        IsRemote: false,
        ETag: 'e2eclip',
        RunTimeTicks: RUNTIME_TICKS,
        ReadAtNativeFramerate: false,
        IgnoreDts: false,
        IgnoreIndex: false,
        GenPtsInput: false,
        SupportsTranscoding: true,
        SupportsDirectStream: directPlay,
        SupportsDirectPlay: directPlay,
        IsInfiniteStream: false,
        UseMostCompatibleTranscodingProfile: false,
        RequiresOpening: false,
        RequiresClosing: false,
        RequiresLooping: false,
        SupportsProbing: true,
        VideoType: 'VideoFile',
        MediaStreams: mediaStreams(),
        MediaAttachments: [],
        Formats: [],
        Bitrate: 1_169_000,
        RequiredHttpHeaders: {},
        TranscodingSubProtocol: 'http',
        DefaultAudioStreamIndex: 1,
        DefaultSubtitleStreamIndex: null,
        HasSegments: false,
    };
}

export function item(overrides = {}) {
    return {
        Name: ITEM_NAME,
        ServerId: SERVER_ID,
        Id: ITEM_ID,
        Etag: 'e2eitem',
        DateCreated: '2026-01-01T00:00:00.0000000Z',
        CanDelete: false,
        CanDownload: true,
        HasSubtitles: false,
        Container: 'mp4',
        SortName: 'astrofin smoke clip',
        PremiereDate: '2026-01-01T00:00:00.0000000Z',
        ExternalUrls: [],
        MediaSources: [mediaSource()],
        Path: 'C:/e2e/media/clip.mp4',
        EnableMediaSourceDisplay: true,
        Taglines: [],
        Genres: ['Test'],
        RunTimeTicks: RUNTIME_TICKS,
        PlayAccess: 'Full',
        ProductionYear: 2026,
        IsFolder: false,
        ParentId: LIBRARY_ID,
        Type: 'Movie',
        People: [],
        Studios: [],
        GenreItems: [{ Name: 'Test', Id: 'e2e00000000000000000000000000007' }],
        LocalTrailerCount: 0,
        UserData: userData(),
        ChildCount: 0,
        SpecialFeatureCount: 0,
        DisplayPreferencesId: ITEM_ID,
        Tags: [],
        PrimaryImageAspectRatio: 0.6666666666666666,
        MediaStreams: mediaStreams(),
        VideoType: 'VideoFile',
        ImageTags: { Primary: 'e2eitemprimary' },
        BackdropImageTags: [],
        ImageBlurHashes: {},
        Chapters: [],
        Trickplay: {},
        LocationType: 'FileSystem',
        MediaType: 'Video',
        Width: 640,
        Height: 360,
        LockedFields: [],
        LockData: false,
        ...overrides,
    };
}

export function itemsResult(items) {
    return { Items: items, TotalRecordCount: items.length, StartIndex: 0 };
}

/** Jellyfin's AllThemeMediaResult: three empty ThemeMediaResults with owners. */
export function themeMedia(ownerId = ITEM_ID) {
    const empty = { Items: [], TotalRecordCount: 0, StartIndex: 0, OwnerId: ownerId };
    return {
        ThemeVideosResult: { ...empty },
        ThemeSongsResult: { ...empty },
        SoundtrackSongsResult: { ...empty },
    };
}

export function authenticationResult() {
    return {
        User: user(),
        SessionInfo: sessionInfo(),
        AccessToken: ACCESS_TOKEN,
        ServerId: SERVER_ID,
    };
}

export function sessionInfo(overrides = {}) {
    return {
        PlayState: {
            CanSeek: true,
            IsPaused: false,
            IsMuted: false,
            RepeatMode: 'RepeatNone',
            PlaybackOrder: 'Default',
        },
        AdditionalUsers: [],
        Capabilities: { PlayableMediaTypes: [], SupportedCommands: [], SupportsMediaControl: false },
        RemoteEndPoint: '127.0.0.1',
        PlayableMediaTypes: ['Audio', 'Video'],
        Id: SESSION_ID,
        UserId: USER_ID,
        UserName: USERNAME,
        Client: 'Astrofin',
        LastActivityDate: new Date().toISOString(),
        LastPlaybackCheckIn: new Date().toISOString(),
        DeviceName: 'E2E',
        DeviceId: 'e2e-device',
        ApplicationVersion: '0.0.0',
        IsActive: true,
        SupportsMediaControl: false,
        SupportsRemoteControl: false,
        NowPlayingQueue: [],
        NowPlayingQueueFullItems: [],
        HasCustomDeviceName: false,
        ServerId: SERVER_ID,
        SupportedCommands: [],
        ...overrides,
    };
}

export function brandingConfiguration() {
    return { LoginDisclaimer: '', CustomCss: '', SplashscreenEnabled: false };
}

export function displayPreferences(id = 'usersettings') {
    return {
        Id: id,
        SortBy: 'SortName',
        RememberIndexing: false,
        PrimaryImageHeight: 250,
        PrimaryImageWidth: 250,
        CustomPrefs: {},
        ScrollDirection: 'Horizontal',
        ShowBackdrop: true,
        RememberSorting: false,
        SortOrder: 'Ascending',
        ShowSidebar: false,
        Client: 'emby',
    };
}
