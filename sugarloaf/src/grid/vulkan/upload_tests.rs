use super::{atlas_upload_barriers, image_copy_region};
use ash::vk;

#[test]
fn atlas_barriers_order_previous_reads_copy_and_future_reads() {
    let image = vk::Image::null();
    for initialized in [false, true] {
        let [before, after] = atlas_upload_barriers(initialized, image);
        assert_eq!(
            before.src_stage_mask,
            if initialized {
                vk::PipelineStageFlags2::FRAGMENT_SHADER
            } else {
                vk::PipelineStageFlags2::TOP_OF_PIPE
            }
        );
        assert_eq!(
            before.src_access_mask,
            if initialized {
                vk::AccessFlags2::SHADER_READ
            } else {
                vk::AccessFlags2::empty()
            }
        );
        assert_eq!(
            before.old_layout,
            if initialized {
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
            } else {
                vk::ImageLayout::UNDEFINED
            }
        );
        assert_eq!(before.dst_stage_mask, vk::PipelineStageFlags2::COPY);
        assert_eq!(before.dst_access_mask, vk::AccessFlags2::TRANSFER_WRITE);
        assert_eq!(before.new_layout, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
        assert_eq!(after.src_stage_mask, vk::PipelineStageFlags2::COPY);
        assert_eq!(after.src_access_mask, vk::AccessFlags2::TRANSFER_WRITE);
        assert_eq!(
            after.dst_stage_mask,
            vk::PipelineStageFlags2::FRAGMENT_SHADER
        );
        assert_eq!(after.dst_access_mask, vk::AccessFlags2::SHADER_READ);
        assert_eq!(after.old_layout, before.new_layout);
        assert_eq!(after.new_layout, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        for barrier in [before, after] {
            assert_eq!(barrier.image, image);
            assert_eq!(barrier.src_queue_family_index, vk::QUEUE_FAMILY_IGNORED);
            assert_eq!(barrier.dst_queue_family_index, vk::QUEUE_FAMILY_IGNORED);
            assert_eq!(
                barrier.subresource_range.aspect_mask,
                vk::ImageAspectFlags::COLOR
            );
            assert_eq!(barrier.subresource_range.base_mip_level, 0);
            assert_eq!(barrier.subresource_range.level_count, 1);
            assert_eq!(barrier.subresource_range.base_array_layer, 0);
            assert_eq!(barrier.subresource_range.layer_count, 1);
        }
    }
}

#[test]
fn glyph_copies_keep_exact_texel_coordinates_and_tightly_packed_pixels() {
    // R8 and RGBA atlas offsets differ only in byte stride, never texel size,
    // bearings or scaling. No padding, resampling, or coordinate rounding.
    for bpp in [1_u64, 4] {
        let copy = image_copy_region(7 * 9 * bpp, 11, 23, 7, 9);
        assert_eq!(copy.buffer_offset, 7 * 9 * bpp);
        assert_eq!(copy.buffer_row_length, 0);
        assert_eq!(copy.buffer_image_height, 0);
        assert_eq!(copy.image_offset, vk::Offset3D { x: 11, y: 23, z: 0 });
        assert_eq!(
            copy.image_extent,
            vk::Extent3D {
                width: 7,
                height: 9,
                depth: 1
            }
        );
        assert_eq!(
            copy.image_subresource.aspect_mask,
            vk::ImageAspectFlags::COLOR
        );
        assert_eq!(copy.image_subresource.mip_level, 0);
        assert_eq!(copy.image_subresource.base_array_layer, 0);
        assert_eq!(copy.image_subresource.layer_count, 1);
    }
}
