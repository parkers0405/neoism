"""Local-only asset repair. Requires Pillow, numpy, scipy; never downloads artwork.

Usage: python rekey_fist_bump.py ORIGINAL.gif OUTPUT_DIRECTORY REVIEW_DIRECTORY
The original is read-only. Only derived fist-bump-transparent.webp/.gif are replaced.
"""
from pathlib import Path
import hashlib
import json
import sys
import numpy as np
from PIL import Image, ImageDraw
from scipy.ndimage import binary_fill_holes, binary_propagation, distance_transform_edt, gaussian_filter, label


def key(rgb, plate):
    dominance = rgb[..., 1] - rgb[..., 0]
    # GIF palette quantization makes real highlights equal-channel or G=R+1.
    # They are not green screen. The old strict R>G test punched them out.
    highlights = (rgb[..., 0] >= 235) & (dominance <= 4) & (rgb[..., 2] <= rgb[..., 0] + 4)
    solid = (dominance < 0) | highlights
    warm = (rgb[..., 0] >= 180) & (dominance <= 8) & (rgb[..., 2] <= rgb[..., 0] + 8)
    # Fill only color-supported enclosed matte holes, never enclosed green gaps
    # between fingers or where the two hands touch.
    solid |= binary_fill_holes(solid) & warm
    seeds = np.zeros(solid.shape, dtype=bool)
    seeds[[0, -1], :] = True
    seeds[:, [0, -1]] = True
    exterior = binary_propagation(seeds & ~solid, mask=~solid)
    # The GIF also dithers a few screen-green palette entries into hand shadows.
    # Repair only tiny fully enclosed speckles, separated from the exterior by
    # foreground; never bridge exterior-connected notches or larger finger gaps.
    components, count = label(~solid & ~exterior)
    sizes = np.bincount(components.ravel())
    enclosed_depth = distance_transform_edt(~exterior)
    repair = np.zeros(solid.shape,dtype=bool)
    for component in range(1,count+1):
        region = components == component
        if sizes[component] <= 16 and enclosed_depth[region].min() > 2:
            repair |= region
    color = rgb.copy()
    if repair.any():
        _, nearest = distance_transform_edt(~solid,return_indices=True)
        color[repair] = rgb[nearest[0][repair],nearest[1][repair]]
        solid |= repair
    alpha = np.zeros(solid.shape)
    if solid.any():
        distance, indices = distance_transform_edt(~solid, return_indices=True)
        foreground = rgb[indices[0], indices[1]]
        delta = foreground - plate
        estimate = np.clip(np.sum((rgb - plate) * delta, axis=2) / np.maximum(np.sum(delta * delta, axis=2), 1), 0, 1)
        # Exterior and enclosed genuine green gaps both retain a narrow soft edge.
        background = exterior | (~solid & ~warm)
        edge = background & (distance <= 3) & (dominance < 70)
        alpha[solid] = 1
        alpha[edge] = estimate[edge]
        alpha[alpha < 0.06] = 0
        unmixed = (rgb - (1 - alpha[..., None]) * plate) / np.maximum(alpha[..., None], 0.01)
        color[edge] = unmixed[edge]
        color[..., 1] = np.minimum(color[..., 1], color[..., 0])
    rgba = np.dstack((np.clip(color, 0, 255), alpha * 255)).round().astype('uint8')
    rgba[rgba[..., 3] == 0, :3] = 0
    return rgba, highlights, solid


def main():
    source, dest, review = map(Path, sys.argv[1:])
    assert source.name not in ('fist-bump-transparent.gif', 'fist-bump-transparent.webp')
    original_hash = hashlib.sha256(source.read_bytes()).hexdigest()
    dest.mkdir(parents=True, exist_ok=True)
    review.mkdir(parents=True, exist_ok=True)
    image = Image.open(source)
    plate = gaussian_filter(np.asarray(image.convert('RGB'), dtype=float), (2, 2, 0))
    frames, durations, originals, stats = [], [], [], []
    for i in range(image.n_frames):
        image.seek(i)
        rgb = np.asarray(image.convert('RGB'), dtype=float)
        rgba, highlights, solid = key(rgb, plate)
        # Actual-source regression, applied to EVERY frame, not a mock hand.
        assert np.all(rgba[highlights, 3] == 255), f'highlight hole in source frame {i}'
        assert np.all(rgba[solid, 3] == 255), f'foreground hole in source frame {i}'
        green = (rgb[..., 1] - rgb[..., 0] >= 70) & ~solid
        assert np.all(rgba[green, 3] == 0), f'green backing in source frame {i}'
        originals.append(Image.fromarray(rgb.astype('uint8')))
        frames.append(Image.fromarray(rgba))
        durations.append(image.info.get('duration', 30))
        stats.append({'frame': i, 'opaque_highlights': int(highlights.sum()), 'soft_pixels': int(((rgba[...,3]>0)&(rgba[...,3]<255)).sum())})
    webp = dest / 'fist-bump-transparent.webp'
    frames[0].save(webp, save_all=True, append_images=frames[1:], duration=durations, loop=0, lossless=True, method=6, minimize_size=False)
    indexed = []
    for frame in frames:
        q = frame.convert('RGB').quantize(colors=255, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
        pixels = np.asarray(q).copy()
        pixels[np.asarray(frame)[...,3] < 128] = 255
        out = Image.fromarray(pixels).convert('P')
        palette = q.getpalette()[:765]
        out.putpalette(palette + [0] * (768-len(palette)))
        out.info['transparency'] = 255
        indexed.append(out)
    indexed[0].save(dest / 'fist-bump-transparent.gif', save_all=True, append_images=indexed[1:], duration=durations, loop=0, transparency=255, disposal=2, optimize=False)
    # Independent libwebp/Pillow decode: compare the full 128-frame source timeline
    # with the optimized encoded timeline (consecutive identical blanks coalesce).
    encoded = Image.open(webp)
    decoded, encoded_durations = [], []
    for i in range(encoded.n_frames):
        encoded.seek(i)
        decoded.append(np.asarray(encoded.convert('RGBA')))
        encoded_durations.append(encoded.info['duration'])
    assert sum(encoded_durations) == sum(durations) == 3840
    end = np.cumsum(encoded_durations)
    t = 0
    for i, frame in enumerate(frames):
        actual = decoded[np.searchsorted(end, t, side='right')]
        expected = np.asarray(frame)
        assert np.array_equal(actual[...,3],expected[...,3]), f'encoded alpha/disposal mismatch frame {i}'
        opaque = expected[...,3] == 255
        assert np.array_equal(actual[opaque],expected[opaque]), f'encoded interior mismatch frame {i}'
        t += durations[i]
    assert not decoded[0][...,3].any() and not decoded[-1][...,3].any()
    gif = Image.open(dest / 'fist-bump-transparent.gif')
    gif_alpha, gif_durations = [], []
    for i in range(gif.n_frames):
        gif.seek(i)
        gif_alpha.append(np.asarray(gif.convert('RGBA'))[...,3])
        gif_durations.append(gif.info['duration'])
    assert sum(gif_durations) == sum(durations)
    gif_end = np.cumsum(gif_durations)
    t = 0
    for i, frame in enumerate(frames):
        assert np.array_equal(gif_alpha[np.searchsorted(gif_end,t,side='right')] > 0,np.asarray(frame)[...,3] >= 128), f'GIF disposal/mask mismatch {i}'
        t += durations[i]
    reference = review / 'source-mattes'
    reference.mkdir(exist_ok=True)
    source_end = np.cumsum(durations)
    t = 0
    for i, duration in enumerate(encoded_durations):
        frames[np.searchsorted(source_end,t,side='right')].save(reference / f'{i:03}.png')
        t += duration
    # All original frames over contrasting checkerboard, plus trouble-frame zooms.
    yy, xx = np.indices((400,400))
    tile = np.where(((xx//16+yy//16)%2)[...,None], [65,75,100], [210,200,180]).astype('uint8')
    sheet = Image.new('RGB',(1600,16*210))
    for i, frame in enumerate(frames):
        base = Image.fromarray(tile).convert('RGBA')
        base.alpha_composite(frame)
        sheet.paste(base.convert('RGB').resize((200,200)),(i%8*200,i//8*210))
        ImageDraw.Draw(sheet).text((i%8*200+4,i//8*210+200),str(i),fill='white')
    sheet.save(review / 'all-128-frames-checker.png')
    selected = [23,25,64,80,114,115,116,118]
    for offset, group in enumerate([selected[:4], selected[4:]]):
        sheet = Image.new('RGB',(1600,3*410))
        for col,i in enumerate(group):
            for row in range(3):
                base = originals[i].convert('RGBA') if row==0 else (Image.fromarray(tile).convert('RGBA') if row==1 else Image.new('RGBA',(400,400),'#15151b'))
                if row: base.alpha_composite(frames[i])
                sheet.paste(base.convert('RGB'),(col*400,row*410))
                ImageDraw.Draw(sheet).text((col*400+4,row*410+400),f'source frame {i}',fill='white')
        sheet.save(review / f'highlights-{offset}.png')
    assert hashlib.sha256(source.read_bytes()).hexdigest() == original_hash
    report = {'source_sha256':original_hash,'source_frames':len(frames),'encoded_webp_frames':len(decoded),'duration_ms':sum(durations),'verified_source_frames':len(frames),'frames':stats}
    (review / 'verification.json').write_text(json.dumps(report,indent=2))
    print(json.dumps({k:v for k,v in report.items() if k!='frames'},indent=2))


if __name__ == '__main__':
    main()