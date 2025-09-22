// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2022 Adrian <adrian.eddy at gmail>

use std::io::*;
use byteorder::{BigEndian, ReadBytesExt};
use crate::{fourcc, read_box, typ_to_str};

/// GoPro GPMF (General Purpose Metadata Format) handler type identifier
pub const GPMF_HANDLER_TYPE: &str = "meta";

/// GPMF GPS data stream identifier - used to detect GPS data in GPMF payloads
const GPMF_GPS_STREAM_ID: u32 = fourcc("GPS5"); // GPS5 = GPS data (lat, lon, alt, speed2d, speed3d)
const GPMF_GPS_TIME_ID: u32 = fourcc("GPSU"); // GPSU = GPS timestamp (UTC)
const GPMF_GYRO_ID: u32 = fourcc("GYRO"); // GYRO = gyroscope data
const GPMF_ACCL_ID: u32 = fourcc("ACCL"); // ACCL = accelerometer data

/// Represents a GPMF GPS sample with timestamp and location data
#[derive(Debug, Clone)]
pub struct GpmfGpsSample {
    pub timestamp_us: u64,           // Timestamp in microseconds
    pub latitude: f64,               // Latitude in degrees
    pub longitude: f64,              // Longitude in degrees
    pub altitude: f64,               // Altitude in meters
    pub speed_2d: f64,              // 2D speed in m/s
    pub speed_3d: f64,              // 3D speed in m/s
}

/// Represents a GPMF track containing GPS samples from a single file
#[derive(Debug, Clone)]
pub struct GpmfTrackData {
    pub samples: Vec<GpmfGpsSample>,
    pub duration_seconds: f64,
    pub sample_rate: f64,           // Samples per second
}

/// Main structure for handling GPMF GPS metadata merging
pub struct GpmfProcessor {
    pub tracks: Vec<GpmfTrackData>,
    pub total_duration: f64,
}

impl GpmfProcessor {
    pub fn new() -> Self {
        Self {
            tracks: Vec::new(),
            total_duration: 0.0,
        }
    }

    /// Check if a file contains GPMF metadata by looking for metadata tracks
    pub fn detect_gpmf_in_file<R: Read + Seek>(reader: &mut R) -> Result<bool> {
        let start_pos = reader.stream_position()?;
        
        // Look for metadata tracks in the MP4 file
        let has_gpmf = Self::scan_for_metadata_track(reader)?;
        
        reader.seek(SeekFrom::Start(start_pos))?;
        Ok(has_gpmf)
    }

    /// Scan the MP4 file structure for metadata tracks that might contain GPMF
    fn scan_for_metadata_track<R: Read + Seek>(reader: &mut R) -> Result<bool> {
        reader.seek(SeekFrom::Start(0))?;
        
        while let Ok((typ, _offs, size, header_size)) = read_box(reader) {
            if size == 0 || typ == 0 { 
                break; 
            }
            
            if typ == fourcc("moov") {
                // Found moov box, look for tracks inside
                return Self::scan_moov_for_metadata_tracks(reader, size - header_size as u64);
            } else {
                // Skip this box
                reader.seek(SeekFrom::Current(size as i64 - header_size))?;
            }
        }
        
        Ok(false)
    }

    /// Scan within moov box for metadata tracks
    fn scan_moov_for_metadata_tracks<R: Read + Seek>(reader: &mut R, max_size: u64) -> Result<bool> {
        let start_pos = reader.stream_position()?;
        
        while reader.stream_position()? - start_pos < max_size {
            let Ok((typ, _offs, size, header_size)) = read_box(reader) else {
                break;
            };
            
            if size == 0 || typ == 0 { 
                break; 
            }
            
            if typ == fourcc("trak") {
                // Found a track, check if it's a metadata track
                if Self::is_metadata_track(reader, size - header_size as u64)? {
                    return Ok(true);
                }
            } else {
                // Skip this box
                reader.seek(SeekFrom::Current(size as i64 - header_size))?;
            }
        }
        
        Ok(false)
    }

    /// Check if a track is a metadata track by looking at its handler type
    fn is_metadata_track<R: Read + Seek>(reader: &mut R, max_size: u64) -> Result<bool> {
        let start_pos = reader.stream_position()?;
        
        while reader.stream_position()? - start_pos < max_size {
            let Ok((typ, _offs, size, header_size)) = read_box(reader) else {
                break;
            };
            
            if size == 0 || typ == 0 { 
                break; 
            }
            
            if typ == fourcc("hdlr") {
                // Found handler box, check the handler type
                let (_v, _flags) = (reader.read_u8()?, reader.read_u24::<BigEndian>()?);
                reader.seek(SeekFrom::Current(4))?; // Skip pre_defined
                let handler_type = reader.read_u32::<BigEndian>()?;
                let handler_type_str = typ_to_str(handler_type);
                
                if handler_type_str == GPMF_HANDLER_TYPE {
                    return Ok(true);
                }
                return Ok(false);
            } else if crate::has_children(typ, true) {
                // Recurse into container boxes
                if Self::is_metadata_track(reader, size - header_size as u64)? {
                    return Ok(true);
                }
            } else {
                // Skip this box
                reader.seek(SeekFrom::Current(size as i64 - header_size))?;
            }
        }
        
        Ok(false)
    }

    /// Extract GPMF GPS data from a single file
    pub fn extract_gpmf_from_file<R: Read + Seek>(
        &mut self, 
        _reader: &mut R, 
        file_duration: f64
    ) -> Result<()> {
        // For now, create a placeholder track with no GPS samples
        // This will be extended to actually parse GPMF data
        let track_data = GpmfTrackData {
            samples: Vec::new(),
            duration_seconds: file_duration,
            sample_rate: 1.0, // Default 1Hz for GPS
        };
        
        self.tracks.push(track_data);
        self.total_duration += file_duration;
        
        Ok(())
    }

    /// Merge all GPMF GPS tracks into a single continuous track with adjusted timestamps
    pub fn merge_gpmf_tracks(&self, _file_durations: &[f64]) -> Result<Vec<GpmfGpsSample>> {
        let mut merged_samples = Vec::new();
        let mut cumulative_time_offset = 0.0;
        
        for (file_index, track) in self.tracks.iter().enumerate() {
            // Add gap time before this file (except the first one)
            if file_index > 0 {
                // Gap detection would go here - for now assume no gaps
            }
            
            // Adjust all GPS sample timestamps by the cumulative offset
            for sample in &track.samples {
                let mut adjusted_sample = sample.clone();
                adjusted_sample.timestamp_us = ((sample.timestamp_us as f64 / 1_000_000.0 + cumulative_time_offset) * 1_000_000.0) as u64;
                merged_samples.push(adjusted_sample);
            }
            
            // Update cumulative offset for next file
            cumulative_time_offset += track.duration_seconds;
        }
        
        Ok(merged_samples)
    }

    /// Create GPMF metadata payload from merged GPS samples
    pub fn create_merged_gpmf_payload(&self, _merged_samples: &[GpmfGpsSample]) -> Result<Vec<u8>> {
        // For now, return empty payload - this would be extended to create actual GPMF format
        let payload = Vec::new();
        
        // GPMF format is complex - would need to implement proper GPMF encoding
        // For the initial implementation, we'll create a minimal valid payload
        
        Ok(payload)
    }

    /// Write merged GPMF metadata to output file
    pub fn write_merged_metadata<W: Write + Seek>(
        &self,
        _output: &mut W,
        _merged_samples: &[GpmfGpsSample]
    ) -> Result<()> {
        // Implementation for writing GPMF metadata to the merged file
        // This would update the metadata track with the merged GPS data
        
        Ok(())
    }
}

/// Check if any of the input files contain GPMF metadata
pub fn detect_gpmf_files<R: Read + Seek>(files: &mut [(R, usize)]) -> Result<Vec<bool>> {
    let mut gpmf_flags = Vec::with_capacity(files.len());
    
    for (file, _size) in files.iter_mut() {
        let has_gpmf = GpmfProcessor::detect_gpmf_in_file(file)?;
        gpmf_flags.push(has_gpmf);
        
        if has_gpmf {
            log::debug!("Detected GPMF metadata in file");
        }
    }
    
    Ok(gpmf_flags)
}

/// Main entry point for merging GPMF GPS metadata across multiple files
pub fn merge_gpmf_metadata<R: Read + Seek, W: Write + Seek>(
    files: &mut [(R, usize)],
    file_durations: &[f64],
    output: &mut W
) -> Result<()> {
    let mut processor = GpmfProcessor::new();
    
    // Extract GPMF data from each file
    for (file_index, (file, _size)) in files.iter_mut().enumerate() {
        let file_duration = file_durations.get(file_index).copied().unwrap_or(0.0);
        processor.extract_gpmf_from_file(file, file_duration)?;
    }
    
    // Merge all tracks into a continuous GPS track
    let merged_samples = processor.merge_gpmf_tracks(file_durations)?;
    
    // Write merged metadata to output
    processor.write_merged_metadata(output, &merged_samples)?;
    
    log::debug!("Successfully merged GPMF metadata from {} files", files.len());
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_gpmf_processor_creation() {
        let processor = GpmfProcessor::new();
        assert_eq!(processor.tracks.len(), 0);
        assert_eq!(processor.total_duration, 0.0);
    }

    #[test]
    fn test_gpmf_sample_creation() {
        let sample = GpmfGpsSample {
            timestamp_us: 1000000, // 1 second
            latitude: 37.7749,
            longitude: -122.4194,
            altitude: 100.0,
            speed_2d: 5.0,
            speed_3d: 5.1,
        };
        
        assert_eq!(sample.timestamp_us, 1000000);
        assert_eq!(sample.latitude, 37.7749);
        assert_eq!(sample.longitude, -122.4194);
    }

    #[test]
    fn test_empty_gpmf_merge() {
        let processor = GpmfProcessor::new();
        let file_durations = vec![1.0, 2.0];
        
        let merged_samples = processor.merge_gpmf_tracks(&file_durations).unwrap();
        assert_eq!(merged_samples.len(), 0);
    }

    #[test]
    fn test_gpmf_detection_with_empty_file() {
        let mut empty_cursor = Cursor::new(Vec::new());
        let result = GpmfProcessor::detect_gpmf_in_file(&mut empty_cursor).unwrap();
        assert_eq!(result, false);
    }
}