#import <AVFoundation/AVFoundation.h>

// Prepare a 1.5x version whose decoded rate remains within 60 fps even at 3x effective
// speed. WebKit's accelerated playback path can fall back to keyframes above 2x.
int truongpaper_export_fast_video(const char *source, const char *output) {
    @autoreleasepool {
        NSURL *input = [NSURL fileURLWithFileSystemRepresentation:source isDirectory:NO relativeToURL:nil];
        NSURL *destination = [NSURL fileURLWithFileSystemRepresentation:output isDirectory:NO relativeToURL:nil];
        AVURLAsset *asset = [AVURLAsset URLAssetWithURL:input options:@{ AVURLAssetPreferPreciseDurationAndTimingKey: @YES }];
        AVAssetTrack *video = [[asset tracksWithMediaType:AVMediaTypeVideo] firstObject];
        if (!video || !CMTIME_IS_NUMERIC(video.timeRange.duration) ||
            CMTimeCompare(video.timeRange.duration, kCMTimeZero) <= 0) return 1;
        CMTime duration = video.timeRange.duration;
        CMTimeRange range = CMTimeRangeMake(kCMTimeZero, duration);
        CMTime fastDuration = CMTimeMultiplyByRatio(duration, 2, 3);
        AVMutableComposition *composition = [AVMutableComposition composition];
        AVMutableCompositionTrack *track = [composition addMutableTrackWithMediaType:AVMediaTypeVideo preferredTrackID:kCMPersistentTrackID_Invalid];
        NSError *error = nil;
        if (![track insertTimeRange:video.timeRange ofTrack:video atTime:kCMTimeZero error:&error]) return 2;
        [track scaleTimeRange:range toDuration:fastDuration];

        AVAssetTrack *audio = [[asset tracksWithMediaType:AVMediaTypeAudio] firstObject];
        if (audio) {
            CMTimeRange audioRange = CMTimeRangeGetIntersection(video.timeRange, audio.timeRange);
            if (!CMTIMERANGE_IS_EMPTY(audioRange)) {
                AVMutableCompositionTrack *audioTrack = [composition addMutableTrackWithMediaType:AVMediaTypeAudio preferredTrackID:kCMPersistentTrackID_Invalid];
                CMTime offset = CMTimeSubtract(audioRange.start, video.timeRange.start);
                if (![audioTrack insertTimeRange:audioRange ofTrack:audio atTime:offset error:&error]) return 2;
                [audioTrack scaleTimeRange:range toDuration:fastDuration];
            }
        }

        CGAffineTransform orientation = video.preferredTransform;
        CGRect bounds = CGRectStandardize(CGRectApplyAffineTransform((CGRect){CGPointZero, video.naturalSize}, orientation));
        if (CGRectIsEmpty(bounds) || CGRectIsInfinite(bounds)) return 2;
        double scale = fmin(1.0, 1920.0 / fmax(bounds.size.width, bounds.size.height));
        CGAffineTransform transform = CGAffineTransformConcat(orientation, CGAffineTransformMakeTranslation(-bounds.origin.x, -bounds.origin.y));
        transform = CGAffineTransformConcat(transform, CGAffineTransformMakeScale(scale, scale));
        AVMutableVideoCompositionLayerInstruction *layer = [AVMutableVideoCompositionLayerInstruction videoCompositionLayerInstructionWithAssetTrack:track];
        [layer setTransform:transform atTime:kCMTimeZero];
        AVMutableVideoCompositionInstruction *instruction = [AVMutableVideoCompositionInstruction videoCompositionInstruction];
        instruction.timeRange = CMTimeRangeMake(kCMTimeZero, fastDuration);
        instruction.layerInstructions = @[layer];
        AVMutableVideoComposition *videoComposition = [AVMutableVideoComposition videoComposition];
        videoComposition.instructions = @[instruction];
        videoComposition.frameDuration = CMTimeMake(1, 30);
        videoComposition.renderSize = CGSizeMake(fmax(2.0, floor(bounds.size.width * scale / 2.0) * 2.0),
                                                 fmax(2.0, floor(bounds.size.height * scale / 2.0) * 2.0));
        AVAssetExportSession *exporter = [[AVAssetExportSession alloc] initWithAsset:composition presetName:AVAssetExportPresetHighestQuality];
        if (!exporter) return 3;
        exporter.outputURL = destination;
        exporter.outputFileType = AVFileTypeMPEG4;
        exporter.videoComposition = videoComposition;
        exporter.audioTimePitchAlgorithm = AVAudioTimePitchAlgorithmSpectral;
        exporter.shouldOptimizeForNetworkUse = YES;
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        [exporter exportAsynchronouslyWithCompletionHandler:^{ dispatch_semaphore_signal(done); }];
        dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
        if (exporter.status != AVAssetExportSessionStatusCompleted) {
            NSLog(@"Fast video export failed: %@", exporter.error);
            return 4;
        }
        return 0;
    }
}
