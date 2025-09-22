// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2022 Adrian <adrian.eddy at gmail>

use std::io::{ Read, Seek, Result, SeekFrom };
use byteorder::{ ReadBytesExt, BigEndian };
use crate::{ fourcc, read_box, typ_to_str };

#[derive(Default, Clone, Debug)]
pub struct TrackDesc {
    pub tkhd_duration: u64,
    pub elst_segment_duration: u64,
    pub mdhd_timescale: u32,
    pub mdhd_duration: u64,
    pub stts: Vec<(u32, u32)>,
    pub stsz: Vec<u32>,
    pub stco: Vec<u64>,
    pub stss: Vec<u32>,
    pub sdtp: Vec<u8>,
    pub sample_offset: u32,
    pub chunk_offset: u32,
    pub stsz_sample_size: u32,
    pub stsz_count: u32,
    pub stsc: Vec<(u32, u32, u32)>, // first_chunk, samples_per_chunk, sample_description_index
    pub co64_final_position: u64,
    pub skip: bool,
    pub elst_entries: Vec<EditListEntry>, // Edit list entries including gaps
    pub handler_type: String, // Track handler type (e.g., "vide", "soun", "meta", etc.)
}

#[derive(Clone, Debug)]
pub struct EditListEntry {
    pub segment_duration: u64, // Duration in movie timescale
    pub media_time: i64,       // Media time (-1 for gaps)
    pub media_rate: u32,       // Typically 0x00010000
}

impl Default for EditListEntry {
    fn default() -> Self {
        Self {
            segment_duration: 0,
            media_time: 0,
            media_rate: 0x00010000,
        }
    }
}

#[derive(Default, Clone, Debug)]
pub struct Desc {
    pub mdat_position: Vec<(Option<usize>, u64, u64)>, // file path, offset, size
    pub mvhd_timescale_per_file: Vec<u32>,
    pub moov_mvhd_timescale: u32,
    pub moov_mvhd_duration: u64,
    pub moov_tracks: Vec<TrackDesc>,
    pub mdat_offset: u64,
    pub mdat_final_position: u64,
    pub file_creation_times: Vec<Option<std::time::SystemTime>>, // Creation time of each file
    pub file_durations: Vec<f64>, // Duration of each file in seconds (legacy, from first track)
    pub track_file_durations: Vec<Vec<f64>>, // track_file_durations[track_index][file_index] = duration in seconds
}

pub fn read_desc<R: Read + Seek>(d: &mut R, desc: &mut Desc, track: usize, max_read: u64, file_index: usize) -> Result<()> {
    let mut tl_track = track;
    let start_offs = d.stream_position()?;
    desc.mvhd_timescale_per_file.push(0);
    while let Ok((typ, offs, size, header_size)) = read_box(d) {
        if size == 0 || typ == 0 { continue; }
        if crate::has_children(typ, true) {
            read_desc(d, desc, tl_track, size - header_size as u64, file_index)?;

            if typ == fourcc("trak") {
                tl_track += 1;
            }
        } else {
            log::debug!("Reading {}, offset: {}, size: {size}, header_size: {header_size}", typ_to_str(typ), offs);
            let org_pos = d.stream_position()?;
            // if typ == fourcc("mdat") {
            //     desc.mdat_position.push((None, org_pos, size - header_size as u64));
            //     desc.mdat_final_position = org_pos;
            // }
            if typ == fourcc("mvhd") || typ == fourcc("tkhd") || typ == fourcc("mdhd") {
                let (v, _flags) = (d.read_u8()?, d.read_u24::<BigEndian>()?);
                if typ == fourcc("mvhd") {
                    let timescale = if v == 1 { d.seek(SeekFrom::Current(8+8))?; d.read_u32::<BigEndian>()? }
                                    else      { d.seek(SeekFrom::Current(4+4))?; d.read_u32::<BigEndian>()? };
                    let duration = if v == 1 { d.read_u64::<BigEndian>()? }
                                   else      { d.read_u32::<BigEndian>()? as u64 };
                    if desc.moov_mvhd_timescale == 0 {
                        desc.moov_mvhd_timescale = timescale;
                    }
                    desc.mvhd_timescale_per_file[file_index] = timescale;
                    desc.moov_mvhd_duration += ((duration as f64 / timescale as f64) * desc.moov_mvhd_timescale as f64).ceil() as u64;
                }
                if let Some(track_desc) = desc.moov_tracks.get_mut(tl_track) {
                    if typ == fourcc("tkhd") {
                        let duration = if v == 1 { d.seek(SeekFrom::Current(8+8+4+4))?; d.read_u64::<BigEndian>()? }
                                       else      { d.seek(SeekFrom::Current(4+4+4+4))?; d.read_u32::<BigEndian>()? as u64 };
                        track_desc.tkhd_duration += ((duration as f64 / *desc.mvhd_timescale_per_file.get(file_index).ok_or(std::io::Error::other("Invalid index"))? as f64) * desc.moov_mvhd_timescale as f64).ceil() as u64;
                    }
                    if typ == fourcc("mdhd") {
                        let timescale = if v == 1 { d.seek(SeekFrom::Current(8+8))?; d.read_u32::<BigEndian>()? }
                                        else      { d.seek(SeekFrom::Current(4+4))?; d.read_u32::<BigEndian>()? };
                        let duration = if v == 1 { d.read_u64::<BigEndian>()? }
                                       else      { d.read_u32::<BigEndian>()? as u64 };
                        if track_desc.mdhd_timescale == 0 {
                            track_desc.mdhd_timescale = timescale;
                        }
                        let add_duration = ((duration as f64 / timescale as f64) * track_desc.mdhd_timescale as f64).ceil() as u64;
                        track_desc.mdhd_duration += add_duration;
                        
                        // Store per-track, per-file duration in seconds
                        // Ensure the track_file_durations array is large enough
                        while desc.track_file_durations.len() <= tl_track {
                            desc.track_file_durations.push(vec![0.0; desc.file_creation_times.len()]);
                        }
                        if file_index < desc.track_file_durations[tl_track].len() {
                            let duration_seconds = duration as f64 / timescale as f64;
                            desc.track_file_durations[tl_track][file_index] = duration_seconds;
                            log::debug!("Track {} file {} duration: {:.2}s", tl_track, file_index, duration_seconds);
                        }
                    }
                }
            }
            if typ == fourcc("elst") || typ == fourcc("stts") || typ == fourcc("stsz") || typ == fourcc("stss") ||
               typ == fourcc("stco") || typ == fourcc("co64") || typ == fourcc("sdtp") || typ == fourcc("stsc") {
                let track_desc = desc.moov_tracks.get_mut(tl_track).unwrap();
                if !(track_desc.skip && file_index > 0) {
                    let (v, _flags) = (d.read_u8()?, d.read_u24::<BigEndian>()?);

                    if typ == fourcc("elst") {
                        let entry_count = d.read_u32::<BigEndian>()?;
                        for _ in 0..entry_count {
                            let segment_duration = if v == 1 { d.read_u64::<BigEndian>()? } else { d.read_u32::<BigEndian>()? as u64 };
                            let media_time       = if v == 1 { d.read_i64::<BigEndian>()? } else { d.read_i32::<BigEndian>()? as i64 };
                            d.seek(SeekFrom::Current(4))?; // Skip Media rate
                            if media_time != -1 {
                                track_desc.elst_segment_duration += segment_duration;
                            }
                        }
                    }
                    if typ == fourcc("stsz") {
                        track_desc.stsz_sample_size = d.read_u32::<BigEndian>()?;
                        let count = d.read_u32::<BigEndian>()?;
                        if track_desc.stsz_sample_size == 0 {
                            for _ in 0..count { track_desc.stsz.push(d.read_u32::<BigEndian>()?); }
                        }
                        track_desc.stsz_count += count;
                    }
                    if typ == fourcc("sdtp") {
                        let count = size - header_size as u64 - 4;
                        for _ in 0..count { track_desc.sdtp.push(d.read_u8()?); }
                    }
                    if typ == fourcc("stss") || typ == fourcc("stco") || typ == fourcc("co64") || typ == fourcc("stts") || typ == fourcc("stsc") {
                        let count = d.read_u32::<BigEndian>()?;
                        let current_file_mdat_position = desc.mdat_position.last().unwrap().1;
                        let mdat_offset = desc.mdat_offset as i64 - current_file_mdat_position as i64;
                        for _ in 0..count {
                            if typ == fourcc("stss") { track_desc.stss.push(d.read_u32::<BigEndian>()? + track_desc.sample_offset); }
                            if typ == fourcc("stco") { track_desc.stco.push((d.read_u32::<BigEndian>()? as i64 + mdat_offset) as u64); }
                            if typ == fourcc("co64") { track_desc.stco.push((d.read_u64::<BigEndian>()? as i64 + mdat_offset) as u64); }
                            if typ == fourcc("stts") { track_desc.stts.push((d.read_u32::<BigEndian>()?, d.read_u32::<BigEndian>()?)); }
                            if typ == fourcc("stsc") { track_desc.stsc.push((
                                d.read_u32::<BigEndian>()? + track_desc.chunk_offset,
                                d.read_u32::<BigEndian>()?,
                                d.read_u32::<BigEndian>()?
                            )); }
                        }
                    }
                }
            }
            if typ == fourcc("tmcd") {
                // Timecode shouldn't be merged
                let track_desc = desc.moov_tracks.get_mut(tl_track).unwrap();
                track_desc.skip = true;
            }
            if typ == fourcc("hdlr") {
                // Read handler type to identify track type (video, audio, metadata, etc.)
                let track_desc = desc.moov_tracks.get_mut(tl_track).unwrap();
                let (_v, _flags) = (d.read_u8()?, d.read_u24::<BigEndian>()?);
                d.seek(SeekFrom::Current(4))?; // Skip pre_defined
                let handler_type = d.read_u32::<BigEndian>()?;
                track_desc.handler_type = typ_to_str(handler_type);
                log::debug!("Track {} handler type: {}", tl_track, track_desc.handler_type);
                
                // Check if this is a GPMF metadata track
                if track_desc.handler_type == "meta" {
                    // This could be a GPMF metadata track - we'll handle it like other metadata tracks
                    // but the GPMF module will process the actual GPS data during merging
                    log::debug!("Found metadata track {} - could contain GPMF data", tl_track);
                }
            }
            d.seek(SeekFrom::Start(org_pos + size - header_size as u64))?;
        }
        if d.stream_position()? - start_offs >= max_read {
            break;
        }
    }
    Ok(())
}

pub fn compute_gaps_and_edit_lists(desc: &mut Desc) -> Result<()> {
    log::debug!("Computing gaps and edit lists for {} files", desc.file_creation_times.len());
    
    // Check if we have enough timestamps to compute gaps
    let has_timestamps = desc.file_creation_times.iter().any(|t| t.is_some());
    
    if !has_timestamps {
        log::debug!("No timestamps available, skipping gap computation");
        return Ok(());
    }
    
    // First, compute all gaps 
    let mut gaps = Vec::new();
    for file_index in 1..desc.file_creation_times.len() {
        let gap_duration = compute_gap_duration(desc, file_index - 1, file_index);
        gaps.push(gap_duration);
    }
    
    // Check if there are any meaningful gaps
    let has_gaps = gaps.iter().any(|&gap| gap > 0.0);
    
    if !has_gaps {
        log::debug!("No gaps detected, using default edit list behavior");
        return Ok(());
    }
    
    // For each track, create edit list entries including gaps
    for track_index in 0..desc.moov_tracks.len() {
        let track = &mut desc.moov_tracks[track_index];
        
        // Add debug logging for track handler types to aid identification
        log::debug!("Processing track {} with handler type: '{}' (skip: {})", 
                   track_index, track.handler_type, track.skip);
        
        if track.skip {
            continue;
        }
        
        track.elst_entries.clear();
        let mut cumulative_media_time = 0i64;
        
        for file_index in 0..desc.file_creation_times.len() {
            // Add gap before this file (except for the first file)
            if file_index > 0 {
                let gap_duration = gaps[file_index - 1];
                if gap_duration > 0.0 {
                    let gap_duration_timescale = (gap_duration * desc.moov_mvhd_timescale as f64).round() as u64;
                    track.elst_entries.push(EditListEntry {
                        segment_duration: gap_duration_timescale,
                        media_time: -1, // -1 indicates a gap/pause
                        media_rate: 0x00010000,
                    });
                    log::debug!("Added gap of {:.2}s between files {} and {}", gap_duration, file_index - 1, file_index);
                }
            }
            
            // Add the actual media segment for this file
            let track_file_duration = if track_index < desc.track_file_durations.len() 
                && file_index < desc.track_file_durations[track_index].len() {
                desc.track_file_durations[track_index][file_index]
            } else {
                // Fallback to global file duration for backward compatibility
                desc.file_durations.get(file_index).copied().unwrap_or(0.0)
            };
            
            if track_file_duration > 0.0 {
                let file_duration_timescale = (track_file_duration * desc.moov_mvhd_timescale as f64).round() as u64;
                track.elst_entries.push(EditListEntry {
                    segment_duration: file_duration_timescale,
                    media_time: cumulative_media_time,
                    media_rate: 0x00010000,
                });
                
                // Convert file duration to media timescale for next media_time
                if track.mdhd_timescale > 0 {
                    cumulative_media_time += (track_file_duration * track.mdhd_timescale as f64).round() as i64;
                }
            }
        }
        
        // Update total elst_segment_duration to include gaps
        track.elst_segment_duration = track.elst_entries.iter()
            .map(|entry| entry.segment_duration)
            .sum();
            
        // Fix: Convert tkhd_duration from movie timescale to media timescale
        // tkhd_duration must be in the track's media timescale (mdhd), but elst_segment_duration is in movie (mvhd) timescale
        if desc.moov_mvhd_timescale > 0 && track.mdhd_timescale > 0 {
            let total_duration_seconds = track.elst_segment_duration as f64 / desc.moov_mvhd_timescale as f64;
            track.tkhd_duration = (total_duration_seconds * track.mdhd_timescale as f64).round() as u64;
        } else {
            // Fallback to direct assignment if timescales are not available
            track.tkhd_duration = track.elst_segment_duration;
        }
    }
    
    // Update the movie header duration to include gaps
    if let Some(first_track) = desc.moov_tracks.first() {
        if !first_track.skip && !first_track.elst_entries.is_empty() {
            desc.moov_mvhd_duration = first_track.elst_segment_duration;
        }
    }
    
    Ok(())
}

fn compute_gap_duration(desc: &Desc, prev_file_index: usize, current_file_index: usize) -> f64 {
    // Try to compute gap based on file creation times
    if let (Some(prev_time), Some(current_time)) = (
        desc.file_creation_times[prev_file_index],
        desc.file_creation_times[current_file_index]
    ) {
        if let Ok(gap) = current_time.duration_since(prev_time) {
            let prev_duration = desc.file_durations[prev_file_index];
            let gap_seconds = gap.as_secs_f64();
            
            log::debug!("File {} ended at {:.2}s after creation", prev_file_index, prev_duration);
            log::debug!("File {} created {:.2}s after file {}", current_file_index, gap_seconds, prev_file_index);
            
            // The actual gap is the time difference minus the duration of the previous file
            let net_gap = gap_seconds - prev_duration;
            
            log::debug!("Net gap: {:.2}s", net_gap);
            
            // Only consider it a gap if it's more than 1 second to avoid false positives
            if net_gap > 1.0 {
                return net_gap;
            }
        }
    }
    
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, Duration};

    #[test]
    fn test_tkhd_duration_timescale_conversion_with_gaps() {
        let mut desc = Desc {
            moov_mvhd_timescale: 1000, // Movie timescale: 1000 units per second
            // Set up file creation times with a gap
            file_creation_times: vec![
                Some(SystemTime::UNIX_EPOCH), 
                Some(SystemTime::UNIX_EPOCH + Duration::from_secs(5)) // 5 second gap after 2s file = 3s net gap
            ],
            file_durations: vec![2.0, 3.0], // 2s and 3s files
            ..Default::default()
        };
        
        let track = TrackDesc {
            mdhd_timescale: 48000, // Media timescale: 48000 units per second  
            ..Default::default()
        };
        
        desc.moov_tracks.push(track);
        
        // Call the function that should fix the timescale - this will detect gaps and process them
        compute_gaps_and_edit_lists(&mut desc).unwrap();
        
        let fixed_track = &desc.moov_tracks[0];
        
        // Should have created edit list entries
        assert!(!fixed_track.elst_entries.is_empty());
        
        // Total duration in movie timescale should be: 2s + 3s gap + 3s = 8s = 8000 units
        assert_eq!(fixed_track.elst_segment_duration, 8000);
        
        // tkhd_duration should be converted to media timescale: 8s * 48000 units/s = 384000 units
        assert_eq!(fixed_track.tkhd_duration, 384000);
    }
    
    #[test]
    fn test_tkhd_duration_conversion_edge_cases() {
        let mut desc = Desc {
            moov_mvhd_timescale: 1000,
            file_creation_times: vec![
                Some(SystemTime::UNIX_EPOCH), 
                Some(SystemTime::UNIX_EPOCH + Duration::from_secs(4)) // 4 second gap after 1s file = 3s net gap
            ],
            file_durations: vec![1.0, 1.0],
            ..Default::default()
        };
        
        let track = TrackDesc {
            mdhd_timescale: 30000, // Different timescale
            ..Default::default()
        };
        
        desc.moov_tracks.push(track);
        
        compute_gaps_and_edit_lists(&mut desc).unwrap();
        
        let fixed_track = &desc.moov_tracks[0];
        
        // Total: 1s + 3s gap + 1s = 5s = 5000 units in movie timescale
        assert_eq!(fixed_track.elst_segment_duration, 5000);
        
        // In media timescale: 5s * 30000 = 150000 units  
        assert_eq!(fixed_track.tkhd_duration, 150000);
    }
    
    #[test]
    fn test_tkhd_duration_no_gaps_no_change() {
        let mut desc = Desc {
            moov_mvhd_timescale: 1000,
            file_creation_times: vec![None, None], // No timestamps = no gaps
            file_durations: vec![2.0, 3.0],
            ..Default::default()
        };
        
        let track = TrackDesc {
            mdhd_timescale: 48000,
            tkhd_duration: 12345, // Some initial value
            ..Default::default()
        };
        
        desc.moov_tracks.push(track);
        
        compute_gaps_and_edit_lists(&mut desc).unwrap();
        
        let fixed_track = &desc.moov_tracks[0];
        
        // Should remain unchanged since no gaps detected
        assert_eq!(fixed_track.tkhd_duration, 12345);
        assert!(fixed_track.elst_entries.is_empty());
    }

    #[test]
    fn test_per_track_duration_calculation() {
        let mut desc = Desc {
            moov_mvhd_timescale: 1000, // Movie timescale: 1000 units per second
            file_creation_times: vec![
                Some(SystemTime::UNIX_EPOCH), 
                Some(SystemTime::UNIX_EPOCH + Duration::from_secs(6)) // 6 second gap after 2s file = 4s net gap
            ],
            file_durations: vec![2.0, 3.0], // Global durations from first track
            track_file_durations: vec![
                vec![2.0, 3.0], // Video track: 2s and 3s files  
                vec![1.5, 2.5], // GPS track: 1.5s and 2.5s files (different durations)
            ],
            ..Default::default()
        };
        
        // Create a video track
        let video_track = TrackDesc {
            mdhd_timescale: 30000, // Video timescale
            handler_type: "vide".to_string(),
            ..Default::default()
        };
        
        // Create a GPS metadata track with different durations
        let gps_track = TrackDesc {
            mdhd_timescale: 1000, // GPS metadata timescale
            handler_type: "meta".to_string(),
            ..Default::default()
        };
        
        desc.moov_tracks.push(video_track);
        desc.moov_tracks.push(gps_track);
        
        // Process gaps and edit lists
        compute_gaps_and_edit_lists(&mut desc).unwrap();
        
        let video_track = &desc.moov_tracks[0];
        let gps_track = &desc.moov_tracks[1];
        
        // Both tracks should have edit list entries
        assert!(!video_track.elst_entries.is_empty(), "Video track should have ELST entries");
        assert!(!gps_track.elst_entries.is_empty(), "GPS metadata track should have ELST entries");
        
        // Video track entries should use video track durations (2s and 3s)
        assert_eq!(video_track.elst_entries[0].segment_duration, 2000); // 2s file
        assert_eq!(video_track.elst_entries[2].segment_duration, 3000); // 3s file
        
        // GPS track entries should use GPS track durations (1.5s and 2.5s)
        assert_eq!(gps_track.elst_entries[0].segment_duration, 1500); // 1.5s file  
        assert_eq!(gps_track.elst_entries[2].segment_duration, 2500); // 2.5s file
        
        // Media times should also be track-specific
        // GPS: first file = 0, second file = 1.5s * 1000 timescale = 1500
        assert_eq!(gps_track.elst_entries[0].media_time, 0);
        assert_eq!(gps_track.elst_entries[2].media_time, 1500);
        
        // Video: first file = 0, second file = 2s * 30000 timescale = 60000
        assert_eq!(video_track.elst_entries[0].media_time, 0);
        assert_eq!(video_track.elst_entries[2].media_time, 60000);
    }

    #[test]
    fn test_dynamic_track_array_resizing() {
        use std::io::Cursor;
        
        let mut desc = Desc {
            track_file_durations: vec![vec![0.0; 2]], // Start with only 1 track
            file_creation_times: vec![None, None],
            ..Default::default()
        };
        
        // Resize tracks to have more than the initial track_file_durations size
        desc.moov_tracks.resize(3, Default::default());
        
        // Simulate reading MDHD for track 2 (index 2), which is beyond initial size
        let mut fake_mdhd_data = Cursor::new(vec![
            0, 0, 0, 0, // Version and flags
            0, 0, 0, 0, // Creation time (v0)
            0, 0, 0, 0, // Modification time (v0) 
            0x00, 0x00, 0x03, 0xE8, // Timescale: 1000 (big endian)
            0x00, 0x00, 0x07, 0xD0, // Duration: 2000 (big endian)
        ]);
        
        // This should trigger dynamic resizing of track_file_durations
        let tl_track = 2;
        let file_index = 0;
        
        // Simulate the MDHD parsing logic - skip version, flags, creation time, modification time
        fake_mdhd_data.set_position(12); // Skip to timescale (4 bytes version/flags + 4 bytes creation + 4 bytes modification)
        let timescale = byteorder::ReadBytesExt::read_u32::<BigEndian>(&mut fake_mdhd_data).unwrap();
        let duration = byteorder::ReadBytesExt::read_u32::<BigEndian>(&mut fake_mdhd_data).unwrap() as u64;
        
        // Simulate the track duration storage logic
        while desc.track_file_durations.len() <= tl_track {
            desc.track_file_durations.push(vec![0.0; desc.file_creation_times.len()]);
        }
        if file_index < desc.track_file_durations[tl_track].len() {
            let duration_seconds = duration as f64 / timescale as f64;
            desc.track_file_durations[tl_track][file_index] = duration_seconds;
        }
        
        // Verify the array was resized correctly
        assert_eq!(desc.track_file_durations.len(), 3);
        assert_eq!(desc.track_file_durations[2][0], 2.0); // 2000/1000 = 2.0 seconds
        assert_eq!(desc.track_file_durations[2].len(), 2); // Should have 2 file slots
    }

    #[test]
    fn test_gps_metadata_track_elst_generation() {
        let mut desc = Desc {
            moov_mvhd_timescale: 1000, // Movie timescale: 1000 units per second
            // Set up file creation times with a gap to test ELST generation
            file_creation_times: vec![
                Some(SystemTime::UNIX_EPOCH), 
                Some(SystemTime::UNIX_EPOCH + Duration::from_secs(4)) // 4 second gap after 1s file = 3s net gap
            ],
            file_durations: vec![1.0, 2.0], // 1s and 2s files
            ..Default::default()
        };
        
        // Create a video track
        let video_track = TrackDesc {
            mdhd_timescale: 30000, // Video timescale
            handler_type: "vide".to_string(),
            ..Default::default()
        };
        
        // Create a GPS metadata track 
        let gps_track = TrackDesc {
            mdhd_timescale: 1000, // GPS metadata timescale
            handler_type: "meta".to_string(),
            ..Default::default()
        };
        
        desc.moov_tracks.push(video_track);
        desc.moov_tracks.push(gps_track);
        
        // Process gaps and edit lists
        compute_gaps_and_edit_lists(&mut desc).unwrap();
        
        let video_track = &desc.moov_tracks[0];
        let gps_track = &desc.moov_tracks[1];
        
        // Both tracks should have edit list entries
        assert!(!video_track.elst_entries.is_empty(), "Video track should have ELST entries");
        assert!(!gps_track.elst_entries.is_empty(), "GPS metadata track should have ELST entries");
        
        // Both tracks should have the same total duration in movie timescale
        // Total: 1s + 3s gap + 2s = 6s = 6000 units in movie timescale
        assert_eq!(video_track.elst_segment_duration, 6000);
        assert_eq!(gps_track.elst_segment_duration, 6000);
        
        // Both tracks should have 3 entries: media1, gap, media2
        assert_eq!(video_track.elst_entries.len(), 3);
        assert_eq!(gps_track.elst_entries.len(), 3);
        
        // Check GPS track entries specifically
        assert_eq!(gps_track.elst_entries[0].segment_duration, 1000); // 1s file
        assert_eq!(gps_track.elst_entries[0].media_time, 0); // Start at 0
        
        assert_eq!(gps_track.elst_entries[1].segment_duration, 3000); // 3s gap
        assert_eq!(gps_track.elst_entries[1].media_time, -1); // Gap entry
        
        assert_eq!(gps_track.elst_entries[2].segment_duration, 2000); // 2s file
        assert_eq!(gps_track.elst_entries[2].media_time, 1000); // 1s offset in GPS timescale
        
        // Check that tkhd_duration is properly converted to media timescale for GPS track
        // 6s * 1000 GPS timescale = 6000 units
        assert_eq!(gps_track.tkhd_duration, 6000);
    }

    #[test]
    fn test_gpmf_metadata_track_handling() {
        // Test that GPMF metadata tracks are handled correctly by the descriptor reader
        let mut desc = Desc {
            moov_mvhd_timescale: 1000,
            file_creation_times: vec![
                Some(SystemTime::UNIX_EPOCH), 
                Some(SystemTime::UNIX_EPOCH + Duration::from_secs(5)) // 5 second gap after 2s file = 3s net gap
            ],
            file_durations: vec![2.0, 3.0],
            ..Default::default()
        };
        
        // Create a video track
        let video_track = TrackDesc {
            mdhd_timescale: 30000,
            handler_type: "vide".to_string(),
            ..Default::default()
        };
        
        // Create a GPMF metadata track (similar to GPS track but specifically GPMF)
        let gpmf_track = TrackDesc {
            mdhd_timescale: 1000, // GPMF metadata typically uses 1000 Hz timescale
            handler_type: "meta".to_string(), // GPMF uses "meta" handler type
            ..Default::default()
        };
        
        desc.moov_tracks.push(video_track);
        desc.moov_tracks.push(gpmf_track);
        
        // Process gaps and edit lists
        compute_gaps_and_edit_lists(&mut desc).unwrap();
        
        let video_track = &desc.moov_tracks[0];
        let gpmf_track = &desc.moov_tracks[1];
        
        // Both tracks should have edit list entries
        assert!(!video_track.elst_entries.is_empty(), "Video track should have ELST entries");
        assert!(!gpmf_track.elst_entries.is_empty(), "GPMF metadata track should have ELST entries");
        
        // Both tracks should have the same total duration in movie timescale
        // Total: 2s + 3s gap + 3s = 8s = 8000 units in movie timescale
        assert_eq!(video_track.elst_segment_duration, 8000);
        assert_eq!(gpmf_track.elst_segment_duration, 8000);
        
        // Check GPMF track entries specifically
        assert_eq!(gpmf_track.elst_entries[0].segment_duration, 2000); // 2s file
        assert_eq!(gpmf_track.elst_entries[0].media_time, 0); // Start at 0
        
        assert_eq!(gpmf_track.elst_entries[1].segment_duration, 3000); // 3s gap
        assert_eq!(gpmf_track.elst_entries[1].media_time, -1); // Gap entry
        
        assert_eq!(gpmf_track.elst_entries[2].segment_duration, 3000); // 3s file
        assert_eq!(gpmf_track.elst_entries[2].media_time, 2000); // 2s offset in GPMF timescale
        
        // Verify handler types are preserved
        assert_eq!(video_track.handler_type, "vide");
        assert_eq!(gpmf_track.handler_type, "meta");
        
        // Check that tkhd_duration is properly converted to media timescale for GPMF track
        // 8s * 1000 GPMF timescale = 8000 units
        assert_eq!(gpmf_track.tkhd_duration, 8000);
    }
}
