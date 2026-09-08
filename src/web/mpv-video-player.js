(function() {
    function getMediaStreamAudioTracks(mediaSource) {
        return mediaSource.MediaStreams.filter(s => s.Type === 'Audio');
    }

    // Convert Jellyfin global MediaStream.Index to 1-based type-relative index
    function getRelativeIndexByType(mediaStreams, jellyIndex, streamType) {
        let relIndex = 1;
        for (const source of mediaStreams) {
            if (source.Type !== streamType || source.IsExternal) continue;
            if (source.Index === jellyIndex) return relIndex;
            relIndex += 1;
        }
        return null;
    }

    function getStreamByIndex(mediaStreams, index) {
        return mediaStreams.find(s => s.Index === index) || null;
    }

    // Session cache for the extra lookups the auto video-mode resolver needs
    // (series items by id, ancestor lists by `ancestors:<parent id>`). A
    // failed fetch caches null so a broken id is not retried on every play.
    const vmItemCache = new Map();

    class mpvVideoPlayer extends window.MpvPlayerBase {
        constructor(args) {
            super(args);
            const { loading, appRouter, globalize, dashboard, playbackManager } = args;
            this.loading = loading;
            this.appRouter = appRouter;
            this.globalize = globalize;
            this.playbackManager = playbackManager;
            if (dashboard && dashboard.default) {
                this.setTransparency = dashboard.default.setBackdropTransparency.bind(dashboard);
            } else {
                this.setTransparency = () => {};
            }

            this.id = 'mpvvideoplayer';
            this.logTag = 'Video';
            this.name = 'MPV Video Player';
            this.syncPlayWrapAs = 'htmlvideoplayer';
            this.priority = -1;
            this.useFullSubtitleUrls = true;
            this.isLocalPlayer = true;
            this.isFetching = false;

            window._mpvVideoPlayerInstance = this;

            this._videoDialog = undefined;
            this._currentSrc = undefined;
            this._timeUpdated = false;
            this._currentPlayOptions = undefined;
            this._endedPending = false;

            // Support jellyfin-web v10.10.7
            this._currentAspectRatio = undefined;

            this.handlers.onPlaying = () => {
                if (!this._started) {
                    this._started = true;
                    this.loading.hide();
                    const dlg = this._videoDialog;
                    // Remove poster so video shows through from subsurface
                    if (dlg) {
                        const poster = dlg.querySelector('.mpvPoster');
                        if (poster) poster.remove();
                    }
                    // "fullscreen" = fills entire web content area, not the actual screen
                    if (this._currentPlayOptions?.fullscreen) {
                        this.appRouter.showVideoOsd();
                        if (dlg) dlg.style.zIndex = 'unset';
                    }
                    window.api.player.setVideoRectangle(0, 0, 0, 0);
                }
                this._emitPlaying();
            };

            this.handlers.onTimeUpdate = (time) => {
                if (time && !this._timeUpdated) this._timeUpdated = true;
                this._seeking = false;
                this._currentTime = time;
                this.events.trigger(this, 'timeupdate');
            };

            this.handlers.onEnded = () => {
                if (!this._endedPending) {
                    this._endedPending = true;
                    this.onEndedInternal();
                }
            };

            this.handlers.onError = (error) => {
                this.removeMediaDialog();
                console.error(`[Media] [${this.logTag}] media error:`, error);
                this.events.trigger(this, 'error', [{ type: 'mediadecodeerror' }]);
            };
        }

        async play(options) {
            // The source badge only learns the play method from playbackstart,
            // which jellyfin-web fires after this promise resolves — up to
            // ~20 s of 4K transcode start-up later. Hand it the answer now.
            // Decoration only: it must never affect playback.
            try { window.AstrofinPlaybackSource?.notePlayOptions?.(options); } catch (e) { /* badge is optional */ }
            console.debug(`[Media] [${this.logTag}] play() called with options:`, options);
            this._started = false;
            this._timeUpdated = false;
            this._currentTime = null;
            this._endedPending = false;
            if (options.resetSubtitleOffset !== false) this.resetSubtitleOffset();
            if (options.fullscreen) this.loading.show();  // fills entire web content area, not the actual screen
            await this.createMediaElement(options);
            await this.applyAutoVideoMode(options);
            console.debug(`[Media] [${this.logTag}] createMediaElement done, calling setCurrentSrc`);
            const result = await this.setCurrentSrc(options);

            // needed when only audio is single external
            const externalAudio = options.mediaSource?.MediaStreams?.find(s => s.Type === 'Audio' && s.IsExternal);
            if (externalAudio && options.playMethod !== 'Transcode') {
                this.setAudioStreamIndex(externalAudio.Index);
            }
            return result;
        }

        get mediaType() { return 'video'; }

        _resolveTracks(options) {
            const streams = options.mediaSource?.MediaStreams || [];
            let defaultAudioIdx = options.mediaSource.DefaultAudioStreamIndex ?? -1;
            const defaultSubIdx = options.mediaSource.DefaultSubtitleStreamIndex ?? -1;

            if (defaultAudioIdx < 0) {
                const fallback = streams.find(s => s.Type === 'Audio' && !s.IsExternal)
                    ?? streams.find(s => s.Type === 'Audio');
                if (fallback) defaultAudioIdx = fallback.Index;
            }

            // Mirror jellyfin-web's UI selection exactly: feed mpv the relative
            // index for DefaultAudioStreamIndex, or TRACK_DISABLE if none is selected.
            // mpv auto track selection is completely disabled as it conflicts with
            // the fact that jellyfin-web is ultimately responsible for that.
            let audioParam = MpvPlayerBase.TRACK_DISABLE;
            let externalAudioUrl = null;
            if (options.playMethod === 'Transcode') {
                // Server bakes the chosen audio into the transcoded output
                // (single audio track in the m3u8). Source MediaStreams indexing
                // doesn't apply — see htmlVideoPlayer/plugin.js:514 for the same
                // logic. Don't audio-add either; audio is already in the stream.
                audioParam = 1;
            } else if (defaultAudioIdx >= 0) {
                const audioStream = getStreamByIndex(streams, defaultAudioIdx);
                if (audioStream && audioStream.DeliveryMethod === 'External' && audioStream.DeliveryUrl) {
                    externalAudioUrl = audioStream.DeliveryUrl;
                } else {
                    const relIdx = getRelativeIndexByType(streams, defaultAudioIdx, 'Audio');
                    audioParam = relIdx != null ? relIdx : MpvPlayerBase.TRACK_DISABLE;
                }
            }

            let subParam = MpvPlayerBase.TRACK_DISABLE;
            let externalSubUrl = null;
            if (defaultSubIdx >= 0) {
                const subStream = getStreamByIndex(streams, defaultSubIdx);
                if (subStream && subStream.DeliveryMethod === 'External' && subStream.DeliveryUrl) {
                    externalSubUrl = subStream.DeliveryUrl;
                } else {
                    const relIdx = getRelativeIndexByType(streams, defaultSubIdx, 'Subtitle');
                    subParam = relIdx != null ? relIdx : MpvPlayerBase.TRACK_DISABLE;
                }
            }

            return { videoParam: 1, audioParam, subParam, externalAudioUrl, externalSubUrl };
        }

        _beforeLoad(options) {
            window.api.player.setAspectMode(options?.aspectRatio || this.getAspectRatio());
        }

        // ---- auto video mode ------------------------------------------------
        //
        // Only runs while the stored setting is `auto`; any other value is a
        // statement about every title and the native side ignores us anyway
        // (it also does so under a `--video-mode` override, which jmpInfo
        // cannot see). Resolution happens here, immediately before loadfile, and is
        // applied transiently: nothing is persisted and the next play resolves
        // again. Every failure degrades to live-action rather than throwing —
        // a metadata lookup must never be able to stop playback.
        async applyAutoVideoMode(options) {
            const item = options?.item || null;
            try {
                const jmp = window.jmpInfo;
                if (jmp?.settings?.playback?.videoMode !== 'auto') return;
                const resolver = window.AstrofinVideoMode;
                if (!resolver) throw new Error('video-mode-resolver.js not loaded');

                const settings = { videoModeLibraries: jmp.videoModeLibraries || {} };
                // The series is only worth a round trip when the item's own
                // tags and genres decided nothing.
                const itemOnly = resolver.resolveVideoMode(item, null, null, settings);
                const series = itemOnly.reason === 'default' && item?.SeriesId
                    ? await this._vmItem(item.SeriesId)
                    : null;
                const library = await this._vmLibrary(item, series);
                const resolved = resolver.resolveVideoMode(item, series, library, settings);
                this._vmApply(resolved.mode, resolved.reason, item);
            } catch (e) {
                const why = (e && e.message) || String(e);
                console.warn(`[Media] [${this.logTag}] auto video mode failed:`, e);
                this._vmApply('live-action', `fallback after error: ${why}`, item);
            }
        }

        _vmApply(mode, reason, item) {
            const name = item?.Name || item?.SeriesName || 'unknown';
            console.info(`[Media] [${this.logTag}] video mode auto -> ${mode} (${reason}) for "${name}"`);
            window.jmpNative?.setPlaybackVideoMode?.(mode, reason, name);
        }

        async _vmItem(id) {
            if (!id) return null;
            if (vmItemCache.has(id)) return vmItemCache.get(id);
            const client = window.ApiClient;
            let fetched = null;
            try {
                if (client?.getItem) fetched = await client.getItem(client.getCurrentUserId(), id);
            } catch (e) {
                console.warn(`[Media] [${this.logTag}] video mode: getItem(${id}) failed:`, e);
            }
            vmItemCache.set(id, fetched || null);
            return fetched || null;
        }

        // The item's top-level library. A ParentId walk cannot find it: a
        // movie's parent is the physical media folder, whose parent is the
        // aggregate root, and the library (CollectionFolder) is only virtual.
        // /Items/{id}/Ancestors?userId= translates that physical folder into
        // the user's CollectionFolder, so the library is simply the first
        // ancestor with a CollectionType. Cached by the nearest shared parent
        // (series for episodes, folder for movies), so one call serves every
        // sibling for the session.
        async _vmLibrary(item, series) {
            if (!item?.Id) return null;
            const key = 'ancestors:' + (series?.Id || item.SeriesId || item.ParentId || item.Id);
            let ancestors = vmItemCache.get(key);
            if (ancestors === undefined) {
                const client = window.ApiClient;
                ancestors = null;
                try {
                    if (client?.getAncestorItems) {
                        ancestors = await client.getAncestorItems(item.Id, client.getCurrentUserId());
                    }
                } catch (e) {
                    console.warn(`[Media] [${this.logTag}] video mode: getAncestorItems(${item.Id}) failed:`, e);
                }
                vmItemCache.set(key, ancestors || null);
            }
            const lib = (ancestors || []).find(a => a && (a.CollectionType || a.Type === 'CollectionFolder' || a.Type === 'UserView'));
            return lib ? { Id: lib.Id, Name: lib.Name } : null;
        }

        setSubtitleStreamIndex(index) {
            if (index == null || index < 0) {
                window.api.player.setSubtitleStream(MpvPlayerBase.TRACK_DISABLE);
                return;
            }
            const streams = this._currentPlayOptions?.mediaSource?.MediaStreams || [];
            const stream = getStreamByIndex(streams, index);
            if (stream && stream.DeliveryMethod === 'External' && stream.DeliveryUrl) {
                window.api.player.addSubtitleStream(stream.DeliveryUrl);
                return;
            }
            const relIdx = getRelativeIndexByType(streams, index, 'Subtitle');
            window.api.player.setSubtitleStream(relIdx != null ? relIdx : MpvPlayerBase.TRACK_DISABLE);
        }

        setSecondarySubtitleStreamIndex(index) {}

        resetSubtitleOffset() {
            this._currentSubtitleOffset = 0;
            this._showSubtitleOffset = false;
            window.api.player.setSubtitleDelay(0);
        }

        enableShowingSubtitleOffset() { this._showSubtitleOffset = true; }
        disableShowingSubtitleOffset() { this._showSubtitleOffset = false; }
        isShowingSubtitleOffsetEnabled() { return this._showSubtitleOffset === true; }
        setSubtitleOffset(offset) {
            const v = parseFloat(offset) || 0;
            this._currentSubtitleOffset = v;
            window.api.player.setSubtitleDelay(Math.round(v * 1000));
        }
        getSubtitleOffset() { return this._currentSubtitleOffset || 0; }

        setAudioStreamIndex(index) {
            if (index == null || index < 0) {
                window.api.player.setAudioStream(MpvPlayerBase.TRACK_DISABLE);
                return;
            }
            const streams = this._currentPlayOptions?.mediaSource?.MediaStreams || [];
            const stream = getStreamByIndex(streams, index);
            if (stream?.IsExternal) {
                // External audio isn't part of the source container and the server
                // doesn't pre-publish a DeliveryUrl for it, so we can't audio-add
                // client-side. Re-enter playbackManager with canSetAudioStreamIndex
                // forced false so it routes through changeStream — the server then
                // regenerates the playback URL with the external audio attached.
                this._forceServerReload = true;
                try {
                    this.playbackManager.setAudioStreamIndex(index, this);
                } finally {
                    this._forceServerReload = false;
                }
                return;
            }
            const relIdx = getRelativeIndexByType(streams, index, 'Audio');
            window.api.player.setAudioStream(relIdx != null ? relIdx : MpvPlayerBase.TRACK_DISABLE);
        }

        stop(destroyPlayer) {
            if (!destroyPlayer && this._videoDialog && this._currentPlayOptions?.backdropUrl) {
                const dlg = this._videoDialog;
                const url = this._currentPlayOptions.backdropUrl;
                if (!dlg.querySelector('.mpvPoster')) {
                    const poster = document.createElement('div');
                    poster.classList.add('mpvPoster');
                    poster.style.cssText = `position:absolute;top:0;left:0;right:0;bottom:0;background:#000 url('${url}') center/cover no-repeat;`;
                    dlg.appendChild(poster);
                }
            }
            window.api.player.stop();
            this.handlers.onEnded();
            if (destroyPlayer) this.destroy();
            return Promise.resolve();
        }

        removeMediaDialog() {
            window.api.player.stop();
            if (window.jmpNative) window.jmpNative.playerOsdActive(false);
            window.api.player.setVideoRectangle(-1, 0, 0, 0);
            document.body.classList.remove('hide-scroll');
            const dlg = this._videoDialog;
            if (dlg) {
                this.setTransparency(0);
                this._videoDialog = null;
                dlg.parentNode.removeChild(dlg);
            }
        }

        destroy() {
            this.removeMediaDialog();
            this.disconnectSignals();

            // Support jellyfin-web v10.10.7
            this._currentAspectRatio = undefined;
        }

        createMediaElement(options) {
            let dlg = document.querySelector('.videoPlayerContainer');
            const isNewDlg = !dlg;
            if (isNewDlg) {
                if (window.jmpNative) window.jmpNative.playerOsdActive(true);
                dlg = document.createElement('div');
                dlg.classList.add('videoPlayerContainer');
                dlg.style.cssText = 'position:fixed;top:0;bottom:0;left:0;right:0;display:flex;align-items:center;background:transparent;';
                if (options.fullscreen) dlg.style.zIndex = 1000;  // fills entire web content area, not the actual screen
                document.body.insertBefore(dlg, document.body.firstChild);
                this._videoDialog = dlg;

                this.connectSignals();
                if (window.jmpNative) {
                    window.jmpNative.notifyRateChange(this._playRate);
                }
            } else {
                this._videoDialog = dlg;
            }

            const existing = dlg.querySelector('.mpvPoster');
            if (existing) existing.remove();
            const poster = document.createElement('div');
            poster.classList.add('mpvPoster');
            const bg = options.backdropUrl
                ? `#000 url('${options.backdropUrl}') center/cover no-repeat`
                : '#000';
            poster.style.cssText = `position:absolute;top:0;left:0;right:0;bottom:0;background:${bg};`;

            const ready = new Promise((resolve) => {
                if (isNewDlg && options.fullscreen) {
                    dlg.style.animation = 'mpv-video-zoomin 240ms ease-in normal';
                    dlg.addEventListener('animationend', resolve, { once: true });
                } else {
                    resolve();
                }
            });
            if (isNewDlg) ready.then(() => this.setTransparency(2));
            dlg.appendChild(poster);

            if (options.fullscreen) document.body.classList.add('hide-scroll');  // fills entire web content area, not the actual screen
            return ready;
        }

        canPlayMediaType(mediaType) {
            return (mediaType || '').toLowerCase() === 'video';
        }
        canPlayItem(item) { return this.canPlayMediaType(item.MediaType); }
        supportsPlayMethod() { return true; }
        static getSupportedFeatures() { return ['PlaybackRate', 'SetAspectRatio']; }
        supports(feature) { return mpvVideoPlayer.getSupportedFeatures().includes(feature); }
        isFullscreen() { return window._isFullscreen === true; }
        toggleFullscreen() {
            if (window.jmpNative) window.jmpNative.toggleFullscreen();
        }

        setPlaybackRate(value) {
            super.setPlaybackRate(value);
            if (window.jmpNative) window.jmpNative.notifyRateChange(value);
        }

        canSetAudioStreamIndex() { return !this._forceServerReload; }
        setPictureInPictureEnabled() {}
        isPictureInPictureEnabled() { return false; }
        isAirPlayEnabled() { return false; }
        setAirPlayEnabled() {}
        setBrightness() {}
        getBrightness() { return 100; }

        togglePictureInPicture() {}
        toggleAirPlay() {}
        getStats() { return Promise.resolve({ categories: [] }); }
        getSupportedAspectRatios() {
            return [
                { id: 'auto',  name: this.globalize.translate('Auto') },
                { id: 'cover', name: this.globalize.translate('AspectRatioCover') },
                { id: 'fill',  name: this.globalize.translate('AspectRatioFill') }
            ];
        }
        getAspectRatio() {
            const aspectRatio = typeof this.appSettings.aspectRatio === 'function'
                ? this.appSettings.aspectRatio()
                // Support jellyfin-web v10.10.7
                : this._currentAspectRatio;

            return aspectRatio || 'auto';
        }
        setAspectRatio(value) {
            if (typeof this.appSettings.aspectRatio === 'function') {
                this.appSettings.aspectRatio(value);
            } else {
                // Support jellyfin-web v10.10.7
                this._currentAspectRatio = value;
            }
            window.api.player.setAspectMode(value);
        }
    }

    window._mpvVideoPlayer = mpvVideoPlayer;
    console.debug('[Media] mpvVideoPlayer class installed');
})();
