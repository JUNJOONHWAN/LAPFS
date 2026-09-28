//! Catalog - the volume file-system (catalog) B-tree. Linkage rules
//! (VIRTUAL via the volume omap; xid = volume superblock o_xid; drec key
//! variant): ctx_search(source:"the APFS specification").
use crate::block_device::BlockDevice;
use crate::btree::BtreeNode;
use crate::container::ContainerError;
use crate::endian::{u16_le, u32_le, ParseError};
use crate::jkey::{JKey, APFS_TYPE_DIR_REC};
use crate::obj::OBJECT_TYPE_BTREE;
use crate::omap::Omap;
use crate::volume::VolumeSuperblock;

const fn oor() -> ContainerError {
    ContainerError::Parse(ParseError::Short {
        at: 0,
        need: 0,
        len: 0,
    })
}

/// Cheap inode metadata for a mount host's getattr (no file-content read).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InodeStat {
    pub size: u64,
    pub is_dir: bool,
    /// POSIX mode (`S_IFREG` | rwx bits). Used by host filesystems to map
    /// `FILE_ATTRIBUTE_READONLY` for Explorer / Get-ItemProperty without
    /// adding an extra read.
    pub mode: u16,
    /// BSD file flags (u32 @ offset 68 in `j_inode_val`). Carries `UF_HIDDEN`
    /// (0x8000), `UF_IMMUTABLE` (0x2), etc. Surfaced so WinFsp can map
    /// `UF_HIDDEN` to `FILE_ATTRIBUTE_HIDDEN`.
    pub bsd_flags: u32,
    /// File timestamps (APFS ns since UNIX epoch). Hosts convert to
    /// Windows FILETIME (100-ns ticks since 1601-01-01) for FileInfo.
    pub create_time: u64,
    pub mod_time: u64,
    pub change_time: u64,
    pub access_time: u64,
}

/// One directory-entry record (APFS_TYPE_DIR_REC).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub file_id: u64,
    pub flags: u16,
}

/// A volume's catalog (file-system) tree, ready to walk.
pub struct Catalog {
    omap: Omap,
    root: BtreeNode,
    xid: u64,
    block_size: u32,
}

impl Catalog {
    /// Open the catalog tree of `vol`.
    /// `vol.omap_oid` (apfs_omap_oid) is a PHYSICAL oid -> `Omap::open` directly,
    /// exactly like the container omap. `vol.root_tree_oid` (apfs_root_tree_oid)
    /// is a VIRTUAL oid -> resolved through the volume omap at the volume
    /// superblock's own xid (`vol.obj.xid`).
    pub fn open<D: BlockDevice>(
        dev: &mut D,
        vol: &VolumeSuperblock,
        block_size: u32,
    ) -> Result<Self, ContainerError> {
        let omap = Omap::open(dev, vol.omap_oid, block_size)?;
        let xid = vol.obj.xid;
        let root_paddr = omap
            .resolve(dev, vol.root_tree_oid, xid, block_size)?
            .ok_or_else(oor)?;
        let bsz = block_size as usize;
        let mut buf = vec![0u8; bsz];
        dev.read_at(
            root_paddr.checked_mul(block_size as u64).ok_or_else(oor)?,
            &mut buf,
        )?;
        let root = BtreeNode::parse(&buf)?;
        if root.obj.object_type() != OBJECT_TYPE_BTREE {
            return Err(ContainerError::Parse(ParseError::BadMagic {
                expected: OBJECT_TYPE_BTREE,
                found: root.obj.object_type(),
            }));
        }
        Ok(Self {
            omap,
            root,
            xid,
            block_size,
        })
    }

    /// Depth-first walk; calls `f(key, value)` for every LEAF entry. Non-leaf
    /// values are 8-byte VIRTUAL child oids resolved through the volume omap.
    #[allow(clippy::type_complexity)]
    fn for_each_leaf<D: BlockDevice>(
        &self,
        dev: &mut D,
        node: &BtreeNode,
        depth: u8,
        f: &mut dyn FnMut(&[u8], &[u8]) -> Result<(), ContainerError>,
    ) -> Result<(), ContainerError> {
        if depth > 32 {
            return Ok(()); // bounded: corrupt/looping tree -> stop, no panic
        }
        for i in 0..node.nkeys {
            if node.is_leaf() {
                let (k, v) = node.var_kv(i)?;
                f(k, v)?;
            } else {
                let (_k, v) = node.var_kv(i)?;
                let child_oid =
                    u64::from_le_bytes(v.get(0..8).ok_or_else(oor)?.try_into().map_err(|_| oor())?);
                let paddr = self
                    .omap
                    .resolve(dev, child_oid, self.xid, self.block_size)?
                    .ok_or_else(oor)?;
                let mut buf = vec![0u8; self.block_size as usize];
                dev.read_at(
                    paddr.checked_mul(self.block_size as u64).ok_or_else(oor)?,
                    &mut buf,
                )?;
                let child = BtreeNode::parse(&buf)?;
                self.for_each_leaf(dev, &child, depth + 1, f)?;
            }
        }
        Ok(())
    }

    /// Visit only catalog branches that can contain this object ID.
    /// APFS catalog keys are ordered by (object ID, record type, suffix).
    /// Adjacent separator IDs are inclusive here: one object's records may
    /// straddle several leaves, including the child before an equal separator.
    #[allow(clippy::type_complexity)]
    fn for_object<D: BlockDevice>(
        &self, dev: &mut D, node: &BtreeNode, depth: u8, object: u64,
        f: &mut dyn FnMut(&[u8], &[u8]) -> Result<(), ContainerError>,
    ) -> Result<(), ContainerError> {
        if depth > 32 { return Err(oor()); }
        if node.is_leaf() {
            if node.level != 0 { return Err(oor()); }
            for i in 0..node.nkeys {
                let (k,v)=node.var_kv(i)?;
                if JKey::parse(k)?.obj_id == object { f(k,v)?; }
            }
            return Ok(());
        }
        if node.level == 0 { return Err(oor()); }
        let mut separators=Vec::with_capacity(node.nkeys.min(512) as usize);
        for i in 0..node.nkeys {
            let (k,_)=node.var_kv(i)?;
            let id=JKey::parse(k)?.obj_id;
            if separators.last().is_some_and(|previous| *previous > id) {return Err(oor());}
            separators.push(id);
        }
        for i in 0..node.nkeys {
            let low=separators[i as usize];
            if low>object {break;}
            if separators.get(i as usize+1).is_some_and(|next| *next<object) {continue;}
            let (_,v)=node.var_kv(i)?;
            let oid=crate::endian::u64_le(v,0)?;
            let paddr=self.omap.resolve(dev,oid,self.xid,self.block_size)?.ok_or_else(oor)?;
            let mut raw=vec![0;self.block_size as usize];
            dev.read_at(paddr.checked_mul(self.block_size as u64).ok_or_else(oor)?,&mut raw)?;
            let child=BtreeNode::parse(&raw)?;
            if child.level.checked_add(1)!=Some(node.level) {return Err(oor());}
            self.for_object(dev,&child,depth+1,object,f)?;
        }
        Ok(())
    }

    /// List the directory entries whose parent inode == `dir_inode`.
    /// `names_hashed` selects the j_drec key variant (see
    /// VolumeSuperblock::names_are_hashed).
    pub fn list_dir<D: BlockDevice>(
        &self,
        dev: &mut D,
        dir_inode: u64,
        names_hashed: bool,
    ) -> Result<Vec<DirEntry>, ContainerError> {
        let mut out: Vec<DirEntry> = Vec::new();
        self.for_object(dev, &self.root, 0, dir_inode, &mut |k, v| {
            let jk = JKey::parse(k)?;
            if jk.obj_type != APFS_TYPE_DIR_REC || jk.obj_id != dir_inode {
                return Ok(());
            }
            // After the 8-byte j_key_t: hashed => u32 name_len_and_hash (name
            // length = low 10 bits), else u16 name_len; then the name bytes
            // (length includes the trailing NUL).
            let (name_off, name_len) = if names_hashed {
                let nlh = u32_le(k, 8)?;
                (12usize, (nlh & 0x0000_03ff) as usize)
            } else {
                let nl = u16_le(k, 8)?;
                (10usize, nl as usize)
            };
            let raw = k.get(name_off..name_off + name_len).ok_or_else(oor)?;
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            let name = String::from_utf8_lossy(raw.get(..end).unwrap_or(&[])).into_owned();
            // j_drec_val { u64 file_id@0; u64 date_added@8; u16 flags@16; ... }
            let file_id =
                u64::from_le_bytes(v.get(0..8).ok_or_else(oor)?.try_into().map_err(|_| oor())?);
            let flags = u16::from_le_bytes(
                v.get(16..18)
                    .ok_or_else(oor)?
                    .try_into()
                    .map_err(|_| oor())?,
            );
            out.push(DirEntry {
                name,
                file_id,
                flags,
            });
            Ok(())
        })?;
        Ok(out)
    }

    /// Resolve a slash-free path given as components (e.g. ["a","b","c"])
    /// to its inode number by descending `list_dir` from the root directory.
    /// Empty `components` → the root inode. Returns `Ok(None)` if any
    /// component is absent. Does not follow symlinks.
    pub fn resolve_path<D: BlockDevice>(
        &self,
        dev: &mut D,
        components: &[&str],
        names_hashed: bool,
    ) -> Result<Option<u64>, ContainerError> {
        let mut ino = crate::jkey::ROOT_DIR_INO_NUM;
        for comp in components {
            let entries = self.list_dir(dev, ino, names_hashed)?;
            match entries.iter().find(|e| e.name == *comp) {
                Some(e) => ino = e.file_id,
                None => return Ok(None),
            }
        }
        Ok(Some(ino))
    }

    /// Assemble the byte content of a data stream identified by `stream_oid`
    /// (a file's private_id, or an xattr's xattr_obj_id): gather the
    /// APFS_TYPE_FILE_EXTENT records keyed by that oid, order by
    /// logical_addr, and truncate to `size`. Uncovered ranges read as zeros.
    fn read_stream<D: BlockDevice>(
        &self,
        dev: &mut D,
        stream_oid: u64,
        size: usize,
    ) -> Result<Vec<u8>, ContainerError> {
        use crate::jkey::APFS_TYPE_FILE_EXTENT;

        let mut extents: Vec<(u64, u64, u64)> = Vec::new(); // (logical, len, paddr)
        self.for_object(dev, &self.root, 0, stream_oid, &mut |k, v| {
            let jk = JKey::parse(k)?;
            if jk.obj_type == APFS_TYPE_FILE_EXTENT && jk.obj_id == stream_oid {
                let logical = u64::from_le_bytes(
                    k.get(8..16)
                        .ok_or_else(oor)?
                        .try_into()
                        .map_err(|_| oor())?,
                );
                let lf =
                    u64::from_le_bytes(v.get(0..8).ok_or_else(oor)?.try_into().map_err(|_| oor())?);
                let paddr = u64::from_le_bytes(
                    v.get(8..16)
                        .ok_or_else(oor)?
                        .try_into()
                        .map_err(|_| oor())?,
                );
                extents.push((logical, lf & 0x00ff_ffff_ffff_ffff, paddr));
            }
            Ok(())
        })?;
        extents.sort_by_key(|e| e.0);

        let mut out = vec![0u8; size];
        for (logical, len, paddr) in extents {
            let logical = logical as usize;
            if logical >= size {
                continue;
            }
            // paddr == 0 encodes a sparse hole (APFS file_extent paddr=0
            // semantics from the APFS specification). The output buffer is
            // already zero-filled, so no disk read is required.
            if paddr == 0 {
                continue;
            }
            let want = std::cmp::min(len as usize, size - logical);
            let mut tmp = vec![0u8; len as usize];
            dev.read_at(
                paddr.checked_mul(self.block_size as u64).ok_or_else(oor)?,
                &mut tmp,
            )?;
            out.get_mut(logical..logical + want)
                .ok_or_else(oor)?
                .copy_from_slice(tmp.get(..want).ok_or_else(oor)?);
        }
        Ok(out)
    }

    /// Read the full uncompressed content of the regular file with the given
    /// inode number (decmpfs-compressed files are out of scope - M3d).
    pub fn read_file<D: BlockDevice>(
        &self,
        dev: &mut D,
        inode_num: u64,
    ) -> Result<Vec<u8>, ContainerError> {
        use crate::inode::Inode;
        use crate::jkey::APFS_TYPE_INODE;

        let mut inode_val: Option<Vec<u8>> = None;
        self.for_object(dev, &self.root, 0, inode_num, &mut |k, v| {
            if inode_val.is_none() {
                let jk = JKey::parse(k)?;
                if jk.obj_type == APFS_TYPE_INODE && jk.obj_id == inode_num {
                    inode_val = Some(v.to_vec());
                }
            }
            Ok(())
        })?;
        let inode = Inode::parse(&inode_val.ok_or_else(oor)?)?;
        let size = inode.dstream.as_ref().map(|d| d.size).unwrap_or(0) as usize;
        self.read_stream(dev, inode.private_id, size)
    }

    /// Read only the requested uncompressed stream range. Never allocate the
    /// full file or an entire extent (both may exceed available memory).
    pub fn read_file_range<D: BlockDevice>(&self, dev: &mut D, ino: u64, offset: u64, len: usize) -> Result<Vec<u8>, ContainerError> {
        use crate::jkey::{APFS_TYPE_INODE, APFS_TYPE_FILE_EXTENT};
        use crate::endian::u64_le;
        let mut raw = None;
        self.for_object(dev, &self.root, 0, ino, &mut |k, v| {
            let key = JKey::parse(k)?;
            if key.obj_id == ino && key.obj_type == APFS_TYPE_INODE { raw = Some(v.to_vec()); }
            Ok(())
        })?;
        let inode = crate::inode::Inode::parse(&raw.ok_or_else(oor)?)?;
        let size = inode.dstream.map(|s| s.size).unwrap_or(0);
        if offset >= size || len == 0 { return Ok(Vec::new()); }
        let want = (size - offset).min(len as u64) as usize;
        let end = offset.checked_add(want as u64).ok_or_else(oor)?;
        let mut extents = Vec::new();
        self.for_object(dev, &self.root, 0, inode.private_id, &mut |k, v| {
            let key = JKey::parse(k)?;
            if key.obj_id == inode.private_id && key.obj_type == APFS_TYPE_FILE_EXTENT {
                let logical = u64_le(k, 8)?; let length_flags = u64_le(v, 0)?;
                let length = length_flags & 0x00ff_ffff_ffff_ffff;
                let stop = logical.checked_add(length).ok_or_else(oor)?;
                if logical < end && stop > offset {
                    if length_flags >> 56 != 0 || u64_le(v, 16)? != 0 { return Err(oor()); }
                    extents.push((logical, stop, u64_le(v, 8)?));
                }
            }
            Ok(())
        })?;
        extents.sort_unstable_by_key(|x| x.0);
        let mut out = vec![0; want]; let mut previous_end = offset;
        for (logical, stop, physical) in extents {
            let begin = logical.max(offset); let finish = stop.min(end);
            if begin < previous_end { return Err(oor()); } previous_end = finish;
            if physical == 0 { continue; } // sparse hole
            let disk = physical.checked_mul(self.block_size as u64).and_then(|p| p.checked_add(begin - logical)).ok_or_else(oor)?;
            dev.read_at(disk, &mut out[(begin-offset) as usize..(finish-offset) as usize])?;
        }
        Ok(out)
    }

    /// Stat an inode WITHOUT reading file content: `is_dir` from the inode
    /// mode, `size` = the com.apple.decmpfs uncompressed_size if the file is
    /// decmpfs-compressed, else the data stream size (0 if none).
    pub fn stat<D: BlockDevice>(
        &self,
        dev: &mut D,
        inode_num: u64,
    ) -> Result<InodeStat, ContainerError> {
        use crate::inode::Inode;
        use crate::jkey::APFS_TYPE_INODE;

        let mut inode_val: Option<Vec<u8>> = None;
        self.for_object(dev, &self.root, 0, inode_num, &mut |k, v| {
            if inode_val.is_none() {
                let jk = JKey::parse(k)?;
                if jk.obj_type == APFS_TYPE_INODE && jk.obj_id == inode_num {
                    inode_val = Some(v.to_vec());
                }
            }
            Ok(())
        })?;
        let inode = Inode::parse(&inode_val.ok_or_else(oor)?)?;
        let is_dir = inode.mode & 0o170000 == 0o040000; // S_IFMT / S_IFDIR
        let size = match self.get_xattr(dev, inode_num, "com.apple.decmpfs")? {
            Some(d) => crate::decmpfs::DecmpfsHeader::parse(&d)
                .map(|h| h.uncompressed_size)
                .unwrap_or(0),
            None => inode.dstream.as_ref().map(|s| s.size).unwrap_or(0),
        };
        Ok(InodeStat {
            size,
            is_dir,
            mode: inode.mode,
            bsd_flags: inode.bsd_flags,
            create_time: inode.create_time,
            mod_time: inode.mod_time,
            change_time: inode.change_time,
            access_time: inode.access_time,
        })
    }

    /// Conservative eligibility check for the external recovery writer.
    /// Shared streams, hard links, compression, sparse files and xattrs are not
    /// part of its validated mutation surface and must fail closed.
    pub fn plain_unshared_file<D: BlockDevice>(&self, dev: &mut D, ino: u64) -> Result<bool, ContainerError> {
        use crate::jkey::{APFS_TYPE_INODE, APFS_TYPE_DSTREAM_ID};
        let mut raw = None; let mut refs = None;
        self.for_object(dev, &self.root, 0, ino, &mut |k, v| {
            let jk = JKey::parse(k)?;
            if jk.obj_id == ino {
                if jk.obj_type == APFS_TYPE_INODE { raw = Some(v.to_vec()); }
                if jk.obj_type == APFS_TYPE_DSTREAM_ID { refs = Some(u32_le(v, 0)?); }
            }
            Ok(())
        })?;
        let raw = raw.ok_or_else(oor)?;
        let inode = crate::inode::Inode::parse(&raw)?;
        crate::inode::parse_unknown_xfields(&raw)?;
        Ok(inode.mode & 0xf000 == 0x8000 && u32_le(&raw, 56)? == 1
            && inode.private_id == ino && inode.internal_flags & 0x44e90 == 0
            && inode.bsd_flags & 0x20 == 0
            && inode.dstream.is_none_or(|d| d.default_crypto_id == 0)
            && refs.is_none_or(|n| n == 1) && self.list_xattrs(dev, ino)?.iter().all(|x| x.name == "com.apple.provenance" && x.flags & 2 != 0 && x.flags & !6 == 0))
    }

    /// Exact layout supported by the bounded in-place range writer. Check this
    /// BEFORE acknowledging a durable queued write, not only during later apply.
    pub fn range_writable_file<D: BlockDevice>(&self, dev: &mut D, ino: u64) -> Result<bool, ContainerError> {
        use crate::jkey::{APFS_TYPE_INODE, APFS_TYPE_DSTREAM_ID, APFS_TYPE_FILE_EXTENT};
        use crate::endian::u64_le;
        if !self.plain_unshared_file(dev, ino)? { return Ok(false); }
        let mut raw = None; let mut refs = None; let mut extents = Vec::new();
        self.for_object(dev, &self.root, 0, ino, &mut |k,v| {
            let j=JKey::parse(k)?;
            if j.obj_type==APFS_TYPE_INODE {raw=Some(v.to_vec());}
            if j.obj_type==APFS_TYPE_DSTREAM_ID {refs=Some(u32_le(v,0)?);}
            if j.obj_type==APFS_TYPE_FILE_EXTENT {extents.push((u64_le(k,8)?,u64_le(v,0)?,u64_le(v,8)?,u64_le(v,16)?));}
            Ok(())
        })?;
        let raw=raw.ok_or_else(oor)?;let inode=crate::inode::Inode::parse(&raw)?;
        if inode.bsd_flags&0x00060006!=0 {return Ok(false);}
        let Some(ds)=inode.dstream else {return Ok(extents.is_empty());};
        let b=self.block_size as u64;
        let allocated=ds.size.checked_add(b-1).ok_or_else(oor)?/b*b;
        if ds.alloced_size!=allocated || (ds.size>0 && refs!=Some(1)) {return Ok(false);}
        let mut covered=0u64;let mut physical=Vec::new();
        for (logical,length,paddr,crypto) in extents {
            if logical!=covered || length==0 || length>>56!=0 || length%b!=0 || paddr==0 || crypto!=0 {return Ok(false);}
            let end=paddr.checked_add(length/b).ok_or_else(oor)?;
            if end>dev.size()/b {return Ok(false);}
            physical.push((paddr,end));covered=covered.checked_add(length).ok_or_else(oor)?;
        }
        physical.sort_unstable();
        if physical.windows(2).any(|w|w[0].1>w[1].0) {return Ok(false);}
        Ok(covered==allocated)
    }

    /// List the extended attributes (name + flags) of the given inode.
    pub fn list_xattrs<D: BlockDevice>(
        &self,
        dev: &mut D,
        inode_num: u64,
    ) -> Result<Vec<crate::xattr::XattrEntry>, ContainerError> {
        use crate::jkey::APFS_TYPE_XATTR;
        use crate::xattr::{xattr_key_name, xattr_val, XattrEntry};

        let mut out: Vec<XattrEntry> = Vec::new();
        self.for_object(dev, &self.root, 0, inode_num, &mut |k, v| {
            let jk = JKey::parse(k)?;
            if jk.obj_type == APFS_TYPE_XATTR && jk.obj_id == inode_num {
                let name = xattr_key_name(k)?;
                let (flags, _xdata) = xattr_val(v)?;
                out.push(XattrEntry { name, flags });
            }
            Ok(())
        })?;
        Ok(out)
    }

    /// Read one extended attribute's raw bytes by name. Embedded values are
    /// returned directly; data-stream values are assembled from the xattr's
    /// FILE_EXTENT records (keyed by xattr_obj_id) and truncated to the
    /// data stream's size. Returns Ok(None) if the attribute is absent.
    /// (Raw bytes only - decmpfs interpretation is M3d.)
    pub fn get_xattr<D: BlockDevice>(
        &self,
        dev: &mut D,
        inode_num: u64,
        name: &str,
    ) -> Result<Option<Vec<u8>>, ContainerError> {
        use crate::jkey::APFS_TYPE_XATTR;
        use crate::xattr::{xattr_key_name, xattr_val, XattrDstream, XATTR_DATA_STREAM};

        let mut hit: Option<(u16, Vec<u8>)> = None;
        self.for_object(dev, &self.root, 0, inode_num, &mut |k, v| {
            if hit.is_none() {
                let jk = JKey::parse(k)?;
                if jk.obj_type == APFS_TYPE_XATTR
                    && jk.obj_id == inode_num
                    && xattr_key_name(k)? == name
                {
                    let (flags, xdata) = xattr_val(v)?;
                    hit = Some((flags, xdata.to_vec()));
                }
            }
            Ok(())
        })?;
        let (flags, xdata) = match hit {
            Some(h) => h,
            None => return Ok(None),
        };
        if flags & XATTR_DATA_STREAM != 0 {
            let ds = XattrDstream::parse(&xdata)?;
            Ok(Some(self.read_stream(
                dev,
                ds.xattr_obj_id,
                ds.size as usize,
            )?))
        } else {
            Ok(Some(xdata))
        }
    }

    /// Read a file's logical content, transparently decompressing
    /// com.apple.decmpfs files (LZVN methods 1/7/8 - M3d). Non-compressed
    /// files fall back to `read_file`.
    pub fn read_file_decompressed<D: BlockDevice>(
        &self,
        dev: &mut D,
        inode_num: u64,
    ) -> Result<Vec<u8>, ContainerError> {
        let dec = self.get_xattr(dev, inode_num, "com.apple.decmpfs")?;
        match dec {
            None => self.read_file(dev, inode_num),
            Some(d) => {
                let rf = self.get_xattr(dev, inode_num, "com.apple.ResourceFork")?;
                crate::decmpfs::decompress(&d, rf.as_deref()).map_err(|e| {
                    ContainerError::Parse(crate::endian::ParseError::Short {
                        at: 0,
                        need: 0,
                        len: format!("{e:?}").len(),
                    })
                })
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
mod tests {
    use super::*;
    use crate::block_device::{BlockError, FileBlockDevice};
    use crate::btree::{BtreeNode, BTNODE_LEAF};
    use crate::checksum::fletcher64;
    use crate::container::Container;
    use crate::jkey::{
        APFS_TYPE_FILE_EXTENT, APFS_TYPE_INODE, OBJ_ID_MASK, OBJ_TYPE_SHIFT, ROOT_DIR_INO_NUM,
    };
    use crate::obj::OBJECT_TYPE_BTREE;
    use crate::omap::{Omap, OmapPhys};
    use std::path::Path;

    // -----------------------------------------------------------------------
    // Regression test: W4-BUG-1 - sparse file hole (paddr=0) must return
    // zeros, not the NX container superblock bytes from physical block 0.
    // -----------------------------------------------------------------------

    /// Minimal in-memory block device for unit tests.
    struct MemDev {
        blocks: std::collections::HashMap<u64, Vec<u8>>,
        block_size: usize,
    }

    impl MemDev {
        fn new(block_size: usize) -> Self {
            Self {
                blocks: std::collections::HashMap::new(),
                block_size,
            }
        }

        /// Fill block at `paddr` with `byte` repeated for the full block.
        fn fill_block(&mut self, paddr: u64, byte: u8) {
            let data = vec![byte; self.block_size];
            self.blocks.insert(paddr * self.block_size as u64, data);
        }

        /// Write arbitrary bytes at byte `offset` (used to plant NX magic).
        fn write_bytes(&mut self, offset: u64, data: &[u8]) {
            let bsz = self.block_size;
            let block_offset = (offset / bsz as u64) * bsz as u64;
            let entry = self
                .blocks
                .entry(block_offset)
                .or_insert_with(|| vec![0u8; bsz]);
            let within = (offset - block_offset) as usize;
            let end = (within + data.len()).min(bsz);
            entry[within..end].copy_from_slice(&data[..end - within]);
        }
    }

    impl BlockDevice for MemDev {
        fn size(&self) -> u64 {
            // Return a nominal size covering at least 64 blocks for test purposes.
            self.block_size as u64 * 64
        }

        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            let bsz = self.block_size as u64;
            let block_base = (offset / bsz) * bsz;
            if let Some(data) = self.blocks.get(&block_base) {
                let within = (offset - block_base) as usize;
                let len = buf.len().min(data.len().saturating_sub(within));
                buf[..len].copy_from_slice(&data[within..within + len]);
                Ok(())
            } else {
                buf.fill(0);
                Ok(())
            }
        }
    }

    fn query_node(level:u16, rows:&[(Vec<u8>,Vec<u8>)]) -> Vec<u8> {
        let mut raw=vec![0u8;4096];
        raw[24..28].copy_from_slice(&OBJECT_TYPE_BTREE.to_le_bytes());
        raw[32..34].copy_from_slice(&(if level==0 {BTNODE_LEAF} else {0}).to_le_bytes());
        raw[34..36].copy_from_slice(&level.to_le_bytes());
        raw[36..40].copy_from_slice(&(rows.len() as u32).to_le_bytes());
        raw[42..44].copy_from_slice(&((rows.len()*8) as u16).to_le_bytes());
        let mut keyoff=0; let mut end=4096;
        for (i,(key,val)) in rows.iter().enumerate() {
            end-=val.len();let toc=56+i*8;
            for (off,n) in [(0,keyoff),(2,key.len()),(4,4096-end),(6,val.len())] {raw[toc+off..toc+off+2].copy_from_slice(&(n as u16).to_le_bytes());}
            let start=56+rows.len()*8+keyoff;raw[start..start+key.len()].copy_from_slice(key);keyoff+=key.len();raw[end..end+val.len()].copy_from_slice(val);
        }
        let sum=fletcher64(&raw);raw[..8].copy_from_slice(&sum.to_le_bytes());raw
    }
    fn query_fixture() -> (Catalog,MemDev) {
        let groups=[vec![1,2,39],vec![40,41,42],vec![42,42],vec![42,43,79],vec![80,99]];
        let mut root=Vec::new();let mut dev=MemDev::new(4096);
        let mut omap=dummy_omap();let mut map=vec![0u8;4096];
        map[24..28].copy_from_slice(&OBJECT_TYPE_BTREE.to_le_bytes());
        map[32..34].copy_from_slice(&(BTNODE_LEAF|crate::btree::BTNODE_FIXED_KV_SIZE).to_le_bytes());
        map[36..40].copy_from_slice(&(groups.len() as u32).to_le_bytes());map[42..44].copy_from_slice(&((groups.len()*4) as u16).to_le_bytes());
        for (i,ids) in groups.iter().enumerate() {
            let oid=100+i as u64;let paddr=10+i as u64;
            let make_key=|id:u64| (id|(8u64<<60)).to_le_bytes().to_vec();
            root.push((make_key(ids[0]),oid.to_le_bytes().to_vec()));
            let rows=ids.iter().enumerate().map(|(j,id)|(make_key(*id),vec![i as u8,j as u8])).collect::<Vec<_>>();
            dev.blocks.insert(paddr*4096,query_node(0,&rows));
            let toc=56+i*4;map[toc..toc+2].copy_from_slice(&((i*16) as u16).to_le_bytes());
            map[toc+2..toc+4].copy_from_slice(&(((i+1)*16) as u16).to_le_bytes());
            let key=56+groups.len()*4+i*16;map[key..key+8].copy_from_slice(&oid.to_le_bytes());map[key+8..key+16].copy_from_slice(&1u64.to_le_bytes());
            let val=4096-(i+1)*16;map[val+4..val+8].copy_from_slice(&4096u32.to_le_bytes());map[val+8..val+16].copy_from_slice(&paddr.to_le_bytes());
        }
        let sum=fletcher64(&map);map[..8].copy_from_slice(&sum.to_le_bytes());omap.root=BtreeNode::parse(&map).unwrap();
        (Catalog{omap,root:BtreeNode::parse(&query_node(1,&root)).unwrap(),xid:1,block_size:4096},dev)
    }
    #[test]
    fn writable_range_preflight_rejects_immutable_sparse_preallocated_shared() {
        use crate::jkey::{APFS_TYPE_INODE,APFS_TYPE_DSTREAM_ID,APFS_TYPE_FILE_EXTENT};
        for (flags,allocated,refs,logical,physical,allowed) in [
            (0u32,8192u64,1u32,0u64,8u64,true),
            (2,8192,1,0,8,false), (0x40000,8192,1,0,8,false),
            (0,16384,1,0,8,false), (0,8192,2,0,8,false),
            (0,8192,1,4096,8,false), (0,8192,1,0,0,false),
        ] {
            let ino=42u64;let mut raw=vec![0u8;140];
            raw[8..16].copy_from_slice(&ino.to_le_bytes());raw[56..60].copy_from_slice(&1u32.to_le_bytes());
            raw[68..72].copy_from_slice(&flags.to_le_bytes());raw[80..82].copy_from_slice(&0x8000u16.to_le_bytes());
            raw[92..94].copy_from_slice(&1u16.to_le_bytes());raw[94..96].copy_from_slice(&40u16.to_le_bytes());
            raw[96]=8;raw[98..100].copy_from_slice(&40u16.to_le_bytes());
            raw[100..108].copy_from_slice(&4103u64.to_le_bytes());raw[108..116].copy_from_slice(&allocated.to_le_bytes());
            let key=|ty:u8|(ino|((ty as u64)<<60)).to_le_bytes().to_vec();
            let mut ek=key(APFS_TYPE_FILE_EXTENT);ek.extend_from_slice(&logical.to_le_bytes());
            let mut ev=vec![0u8;24];ev[..8].copy_from_slice(&allocated.to_le_bytes());ev[8..16].copy_from_slice(&physical.to_le_bytes());
            let rows=vec![(key(APFS_TYPE_INODE),raw),(key(APFS_TYPE_DSTREAM_ID),refs.to_le_bytes().to_vec()),(ek,ev)];
            let cat=Catalog{omap:dummy_omap(),root:BtreeNode::parse(&query_node(0,&rows)).unwrap(),xid:1,block_size:4096};
            assert_eq!(cat.range_writable_file(&mut MemDev::new(4096),ino).unwrap(),allowed,"{flags} {allocated} {refs} {logical} {physical}");
        }
    }

    #[test]
    fn object_query_matches_full_walk_including_equal_separators_and_absent_ids() {
        let (cat,mut dev)=query_fixture();
        let mut all=Vec::new();cat.for_each_leaf(&mut dev,&cat.root,0,&mut |k,v|{all.push((k.to_vec(),v.to_vec()));Ok(())}).unwrap();
        for id in 0..=101 {
            let mut got=Vec::new();cat.for_object(&mut dev,&cat.root,0,id,&mut |k,v|{got.push((k.to_vec(),v.to_vec()));Ok(())}).unwrap();
            let expected=all.iter().filter(|(k,_)|JKey::parse(k).unwrap().obj_id==id).cloned().collect::<Vec<_>>();assert_eq!(got,expected,"object {id}");
        }
    }
    #[test]
    fn object_query_prunes_unrelated_corruption_and_rejects_target_corruption() {
        let (cat,mut dev)=query_fixture();dev.blocks.get_mut(&(10*4096)).unwrap()[100]^=1;
        let mut count=0;cat.for_object(&mut dev,&cat.root,0,42,&mut |_,_|{count+=1;Ok(())}).unwrap();assert_eq!(count,4);
        assert!(cat.for_object(&mut dev,&cat.root,0,2,&mut |_,_|Ok(())).is_err());
        assert!(cat.for_object(&mut dev,&cat.root,33,42,&mut |_,_|Ok(())).is_err());
    }

    #[test]
    fn range_read_over_4gib_never_reads_or_allocates_entire_extent() {
        const SIZE: u64 = 8 * 1024 * 1024 * 1024;
        let ino = 42u64;
        let mut inode = vec![0u8; 140];
        inode[8..16].copy_from_slice(&ino.to_le_bytes());
        inode[80..82].copy_from_slice(&0x8000u16.to_le_bytes());
        inode[92..94].copy_from_slice(&1u16.to_le_bytes());
        inode[94..96].copy_from_slice(&40u16.to_le_bytes());
        inode[96] = 8; inode[98..100].copy_from_slice(&40u16.to_le_bytes());
        inode[100..108].copy_from_slice(&SIZE.to_le_bytes());
        inode[108..116].copy_from_slice(&SIZE.to_le_bytes());
        let key_inode = (ino | ((APFS_TYPE_INODE as u64) << 60)).to_le_bytes().to_vec();
        let mut key_extent = (ino | ((APFS_TYPE_FILE_EXTENT as u64) << 60)).to_le_bytes().to_vec();
        key_extent.extend_from_slice(&0u64.to_le_bytes());
        let mut extent = vec![0u8; 24]; extent[..8].copy_from_slice(&SIZE.to_le_bytes());
        extent[8..16].copy_from_slice(&4u64.to_le_bytes());
        let rows = [(key_inode, inode), (key_extent, extent)];
        let mut raw = vec![0u8; 4096]; raw[24..28].copy_from_slice(&OBJECT_TYPE_BTREE.to_le_bytes());
        raw[32..34].copy_from_slice(&BTNODE_LEAF.to_le_bytes());
        raw[36..40].copy_from_slice(&2u32.to_le_bytes()); raw[42..44].copy_from_slice(&16u16.to_le_bytes());
        let mut ko = 0usize; let mut ve = 4096usize;
        for (i, (key, val)) in rows.iter().enumerate() {
            ve -= val.len(); let toc = 56 + i*8;
            for (off, number) in [(0, ko), (2, key.len()), (4, 4096-ve), (6, val.len())] { raw[toc+off..toc+off+2].copy_from_slice(&(number as u16).to_le_bytes()); }
            raw[72+ko..72+ko+key.len()].copy_from_slice(key); ko += key.len(); raw[ve..ve+val.len()].copy_from_slice(val);
        }
        let checksum = fletcher64(&raw); raw[..8].copy_from_slice(&checksum.to_le_bytes());
        let catalog = Catalog { omap: dummy_omap(), root: BtreeNode::parse(&raw).unwrap(), xid: 1, block_size: 4096 };
        struct Counting { bytes: usize }
        impl BlockDevice for Counting {
            fn size(&self) -> u64 { 10 * 1024 * 1024 * 1024 }
            fn read_at(&mut self, off: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
                assert!(bytes.len() <= 37, "read amplification regression"); self.bytes += bytes.len();
                for (i,b) in bytes.iter_mut().enumerate() { *b = ((off+i as u64)%251) as u8; } Ok(())
            }
        }
        let mut dev = Counting { bytes: 0 }; let offset = (1u64 << 32) + 17;
        let actual = catalog.read_file_range(&mut dev, ino, offset, 37).unwrap();
        assert_eq!(dev.bytes, 37); assert_eq!(actual, (0..37).map(|i| ((4*4096+offset+i)%251) as u8).collect::<Vec<_>>());
        assert_eq!(catalog.read_file_range(&mut dev, ino, SIZE-7, 100).unwrap().len(), 7);
        assert_eq!(dev.bytes, 44); assert!(catalog.read_file_range(&mut dev, ino, SIZE, 100).unwrap().is_empty());
    }

    /// Build a minimal var-kv leaf BtreeNode (validated checksum) containing
    /// `nkeys` FILE_EXTENT records for `stream_oid`. Each entry is:
    ///   `(logical_offset, byte_len, paddr)`.
    ///
    /// Layout (4 KiB block):
    ///   - DATA_BASE = 56
    ///   - toc_off = 0, toc_len = nkeys * 8 (kvloc_t per entry)
    ///   - key area starts at DATA_BASE + toc_len
    ///   - values grow backwards from byte 4095 (leaf, not root → no btree_info)
    fn build_extent_leaf(
        stream_oid: u64,
        extents: &[(u64, u64, u64)], // (logical, len, paddr)
    ) -> Vec<u8> {
        let bsz = 4096usize;
        let data_base: usize = 56;
        let toc_len = extents.len() * 8; // one kvloc_t per entry
        let key_area_start = data_base + toc_len;
        let key_size = 16usize; // 8-byte j_key_t + 8-byte logical_addr
        let val_size = 16usize; // 8-byte len_and_flags + 8-byte paddr
        let val_area_end = bsz; // leaf (not root) → no btree_info_t reservation

        let mut b = vec![0u8; bsz];

        // obj_phys header fields (object_type = BTREE at offset 24)
        b[24..28].copy_from_slice(&OBJECT_TYPE_BTREE.to_le_bytes());
        // btn_flags: BTNODE_LEAF (var-kv, not FIXED_KV)
        b[32..34].copy_from_slice(&BTNODE_LEAF.to_le_bytes());
        // btn_level = 0
        b[34..36].copy_from_slice(&0u16.to_le_bytes());
        // btn_nkeys
        b[36..40].copy_from_slice(&(extents.len() as u32).to_le_bytes());
        // btn_table_space: off=0, len=toc_len
        b[40..42].copy_from_slice(&0u16.to_le_bytes()); // toc_off
        b[42..44].copy_from_slice(&(toc_len as u16).to_le_bytes()); // toc_len

        for (i, &(logical, len, paddr)) in extents.iter().enumerate() {
            // Build j_file_extent_key_t: 8-byte j_key_t + 8-byte logical_addr.
            // j_key_t = (obj_id & OBJ_ID_MASK) | (type << 60)
            let jkey_word =
                (stream_oid & OBJ_ID_MASK) | ((APFS_TYPE_FILE_EXTENT as u64) << OBJ_TYPE_SHIFT);
            let mut key = [0u8; 16];
            key[0..8].copy_from_slice(&jkey_word.to_le_bytes());
            key[8..16].copy_from_slice(&logical.to_le_bytes());

            // j_file_extent_val_t: len_and_flags (low 56 bits = byte_len) + paddr.
            let mut val = [0u8; 16];
            val[0..8].copy_from_slice(&(len & 0x00ff_ffff_ffff_ffff).to_le_bytes());
            val[8..16].copy_from_slice(&paddr.to_le_bytes());

            let k_off = i * key_size; // relative to key_area_start
            let v_abs = val_area_end - (i + 1) * val_size; // absolute byte offset
            let v_off = val_area_end - v_abs; // TOC encodes distance back from val_area_end

            // Write kvloc_t into TOC
            let toc_entry = data_base + i * 8;
            b[toc_entry..toc_entry + 2].copy_from_slice(&(k_off as u16).to_le_bytes());
            b[toc_entry + 2..toc_entry + 4].copy_from_slice(&(key_size as u16).to_le_bytes());
            b[toc_entry + 4..toc_entry + 6].copy_from_slice(&(v_off as u16).to_le_bytes());
            b[toc_entry + 6..toc_entry + 8].copy_from_slice(&(val_size as u16).to_le_bytes());

            // Write key bytes
            let k_start = key_area_start + k_off;
            b[k_start..k_start + key_size].copy_from_slice(&key);

            // Write value bytes at absolute position
            b[v_abs..v_abs + val_size].copy_from_slice(&val);
        }

        // Compute and store Fletcher-64 checksum (obj_phys.cksum at offset 0).
        let ck = fletcher64(&b);
        b[0..8].copy_from_slice(&ck.to_le_bytes());
        b
    }

    /// Build a dummy Omap backed by a trivial leaf node. Its `resolve` method
    /// will never be called because `read_stream` only calls `for_each_leaf`
    /// on `self.root` which is a leaf - no omap resolution needed.
    fn dummy_omap() -> Omap {
        // We need a parseable BtreeNode for Omap::root. Build a minimal
        // FIXED_KV leaf with 0 entries using the same layout as btree tests.
        let bsz = 4096usize;
        let mut b = vec![0u8; bsz];
        b[24..28].copy_from_slice(&OBJECT_TYPE_BTREE.to_le_bytes());
        use crate::btree::BTNODE_FIXED_KV_SIZE;
        b[32..34].copy_from_slice(&(BTNODE_LEAF | BTNODE_FIXED_KV_SIZE).to_le_bytes());
        b[34..36].copy_from_slice(&0u16.to_le_bytes()); // level
        b[36..40].copy_from_slice(&0u32.to_le_bytes()); // nkeys = 0
        b[40..42].copy_from_slice(&0u16.to_le_bytes()); // toc_off
        b[42..44].copy_from_slice(&0u16.to_le_bytes()); // toc_len
        let ck = fletcher64(&b);
        b[0..8].copy_from_slice(&ck.to_le_bytes());
        let root = BtreeNode::parse(&b).expect("dummy omap root");

        // Build a dummy OmapPhys (parse from a valid block to satisfy the
        // parse path, but we won't call resolve so the content is irrelevant).
        // Use a raw block with OBJECT_TYPE_OMAP and valid Fletcher-64.
        use crate::obj::OBJECT_TYPE_OMAP;
        let mut ob = vec![0u8; bsz];
        ob[24..28].copy_from_slice(&OBJECT_TYPE_OMAP.to_le_bytes());
        let ck2 = fletcher64(&ob);
        ob[0..8].copy_from_slice(&ck2.to_le_bytes());
        let phys = OmapPhys::parse(&ob).expect("dummy OmapPhys");

        Omap { phys, root }
    }

    // W4-BUG-1 regression: a sparse extent (paddr=0) must yield zero bytes,
    // not bytes from physical block 0 (the NX container superblock).
    #[test]
    fn sparse_hole_reads_as_zeros_not_nx_superblock() {
        const BSZ: u32 = 4096;
        const STREAM_OID: u64 = 100;
        const NX_MAGIC: &[u8] = b"BSXN"; // NXSB in little-endian bytes as seen on disk

        // Two extents: first is a sparse hole (paddr=0), second has real data.
        let raw_root = build_extent_leaf(
            STREAM_OID,
            &[
                (0, BSZ as u64, 0),          // sparse hole: paddr=0
                (BSZ as u64, BSZ as u64, 5), // real data at paddr=5
            ],
        );

        // Device: block 0 holds NX superblock magic + 0xDE filler,
        // block 5 holds all 0xCC (the "real file content").
        let mut dev = MemDev::new(BSZ as usize);
        // Plant NX magic at physical offset 0 so the pre-fix code would
        // corrupt the output buffer with it.
        dev.fill_block(0, 0xDE);
        dev.write_bytes(0, NX_MAGIC);
        // Plant real file content at block 5.
        dev.fill_block(5, 0xCC);

        let root = BtreeNode::parse(&raw_root).expect("parse root node");
        let cat = Catalog {
            omap: dummy_omap(),
            root,
            xid: 1,
            block_size: BSZ,
        };

        let data = cat
            .read_stream(&mut dev, STREAM_OID, (BSZ as usize) * 2)
            .expect("read_stream");

        // Hole region (first block) must be all zeros - not NX superblock bytes.
        assert_eq!(data.len(), BSZ as usize * 2, "total size");
        assert!(
            data[..BSZ as usize].iter().all(|&b| b == 0),
            "sparse hole must be zero-filled; first non-zero at byte {:?}",
            data[..BSZ as usize].iter().position(|&b| b != 0)
        );
        // NX magic must never appear in the output.
        assert!(
            !data.windows(4).any(|w| w == NX_MAGIC),
            "NX superblock magic must not appear in sparse file read"
        );
        // Real data region (second block) must be 0xCC.
        assert!(
            data[BSZ as usize..].iter().all(|&b| b == 0xCC),
            "real extent region must contain file content bytes"
        );
    }

    #[test]
    fn lists_real_large_fixture_tree() {
        let img = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/apfs-large.img"
        );
        if !Path::new(img).exists() {
            eprintln!("skip: fixture missing - run `cargo run -p xtask -- gen-fixture --large`");
            return;
        }
        let mut dev = FileBlockDevice::open(img).expect("open fixture");
        let c = Container::open(&mut dev).expect("open container");
        let bsz = c.superblock.block_size;
        let vols = c.list_user_volumes(&mut dev).expect("list volumes");
        let vol = vols.first().expect("at least one user volume").clone();

        let cat = Catalog::open(&mut dev, &vol, bsz).expect("open catalog");
        let hashed = vol.names_are_hashed();
        let root = cat
            .list_dir(&mut dev, ROOT_DIR_INO_NUM, hashed)
            .expect("list root dir");

        let names: std::collections::BTreeSet<String> =
            root.iter().map(|e| e.name.clone()).collect();
        let expected_dirs: Vec<String> = (0..16).map(|d| format!("dir{d:02}")).collect();
        if !expected_dirs.iter().all(|d| names.contains(d)) {
            eprintln!(
                "skip: size-only fixture (hdiutil attach fell back) - \
                 re-run `cargo run -p xtask -- gen-fixture --large` on a macOS host. \
                 root names found: {names:?}"
            );
            return;
        }
        // All 16 created directories are present at root.
        for d in &expected_dirs {
            assert!(names.contains(d), "missing root dir {d}");
        }
        // dir00's children are exactly f000.bin..f063.bin.
        let dir00 = root
            .iter()
            .find(|e| e.name == "dir00")
            .expect("dir00 entry");
        let kids = cat
            .list_dir(&mut dev, dir00.file_id, hashed)
            .expect("list dir00");
        let kid_names: std::collections::BTreeSet<String> =
            kids.iter().map(|e| e.name.clone()).collect();
        for f in 0..64 {
            let want = format!("f{f:03}.bin");
            assert!(kid_names.contains(&want), "dir00 missing {want}");
        }
        assert!(
            kid_names.len() >= 64,
            "dir00 should have at least 64 files (M3d adds cmp_* samples), got {}",
            kid_names.len()
        );
    }

    #[test]
    fn reads_real_file_content_4096_a() {
        let img = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/apfs-large.img"
        );
        if !Path::new(img).exists() {
            eprintln!("skip: fixture missing - run `cargo run -p xtask -- gen-fixture --large`");
            return;
        }
        let mut dev = FileBlockDevice::open(img).expect("open fixture");
        let c = Container::open(&mut dev).expect("open container");
        let bsz = c.superblock.block_size;
        let vols = c.list_user_volumes(&mut dev).expect("list volumes");
        let vol = vols.first().expect("user volume").clone();
        let cat = Catalog::open(&mut dev, &vol, bsz).expect("open catalog");
        let hashed = vol.names_are_hashed();

        let root = cat
            .list_dir(&mut dev, ROOT_DIR_INO_NUM, hashed)
            .expect("list root");
        let dir00 = match root.iter().find(|e| e.name == "dir00") {
            Some(d) => d.clone(),
            None => {
                eprintln!(
                    "skip: size-only fixture (hdiutil attach fell back) - \
                     re-run `cargo run -p xtask -- gen-fixture --large` on a macOS host"
                );
                return;
            }
        };
        let kids = cat
            .list_dir(&mut dev, dir00.file_id, hashed)
            .expect("list dir00");
        let f000 = kids
            .iter()
            .find(|e| e.name == "f000.bin")
            .expect("f000.bin entry");

        let content = cat
            .read_file(&mut dev, f000.file_id)
            .expect("read f000.bin");
        assert_eq!(content.len(), 4096, "f000.bin logical size");
        assert!(
            content.iter().all(|&b| b == b'A'),
            "f000.bin must be 4096 bytes of 0x41"
        );
    }

    #[test]
    fn reads_real_xattrs_embedded_and_stream() {
        let img = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/apfs-large.img"
        );
        if !Path::new(img).exists() {
            eprintln!("skip: fixture missing - run `cargo run -p xtask -- gen-fixture --large`");
            return;
        }
        let mut dev = FileBlockDevice::open(img).expect("open fixture");
        let c = Container::open(&mut dev).expect("open container");
        let bsz = c.superblock.block_size;
        let vols = c.list_user_volumes(&mut dev).expect("list volumes");
        let vol = vols.first().expect("user volume").clone();
        let cat = Catalog::open(&mut dev, &vol, bsz).expect("open catalog");
        let hashed = vol.names_are_hashed();

        let root = cat
            .list_dir(&mut dev, ROOT_DIR_INO_NUM, hashed)
            .expect("list root");
        let dir00 = match root.iter().find(|e| e.name == "dir00") {
            Some(d) => d.clone(),
            None => {
                eprintln!(
                    "skip: size-only fixture (hdiutil attach fell back) - \
                     re-run `cargo run -p xtask -- gen-fixture --large` on a macOS host"
                );
                return;
            }
        };
        let kids = cat
            .list_dir(&mut dev, dir00.file_id, hashed)
            .expect("list dir00");
        let f000 = kids
            .iter()
            .find(|e| e.name == "f000.bin")
            .expect("f000.bin entry");

        let names: std::collections::BTreeSet<String> = cat
            .list_xattrs(&mut dev, f000.file_id)
            .expect("list xattrs")
            .into_iter()
            .map(|e| e.name)
            .collect();
        if !names.contains("user.apfsx_small") {
            eprintln!(
                "skip: fixture predates M3c xattrs - re-run \
                 `cargo run -p xtask -- gen-fixture --large` (updated xtask). got: {names:?}"
            );
            return;
        }
        assert!(
            names.contains("user.apfsx_big"),
            "missing user.apfsx_big; got {names:?}"
        );

        let small = cat
            .get_xattr(&mut dev, f000.file_id, "user.apfsx_small")
            .expect("get small")
            .expect("small present");
        assert_eq!(small, b"hello-apfs", "embedded xattr value");

        let big = cat
            .get_xattr(&mut dev, f000.file_id, "user.apfsx_big")
            .expect("get big")
            .expect("big present");
        assert_eq!(big.len(), 5000, "stream xattr logical length");
        assert!(
            big.iter().all(|&b| b == b'Z'),
            "stream xattr content must be 5000 bytes of 'Z'"
        );

        assert!(cat
            .get_xattr(&mut dev, f000.file_id, "user.nope")
            .expect("get absent")
            .is_none());
    }

    #[test]
    fn resolve_path_navigates_real_fixture() {
        let img = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/apfs-large.img"
        );
        if !Path::new(img).exists() {
            eprintln!("skip: fixture missing - run `cargo run -p xtask -- gen-fixture --large`");
            return;
        }
        let mut dev = FileBlockDevice::open(img).expect("open fixture");
        let c = Container::open(&mut dev).expect("open container");
        let bsz = c.superblock.block_size;
        let vol = c
            .list_user_volumes(&mut dev)
            .expect("list volumes")
            .first()
            .expect("user volume")
            .clone();
        let cat = Catalog::open(&mut dev, &vol, bsz).expect("open catalog");
        let hashed = vol.names_are_hashed();

        if cat
            .resolve_path(&mut dev, &["dir00"], hashed)
            .expect("resolve dir00")
            .is_none()
        {
            eprintln!("skip: size-only fixture - regenerate gen-fixture --large");
            return;
        }
        let ino = cat
            .resolve_path(&mut dev, &["dir00", "f000.bin"], hashed)
            .expect("resolve dir00/f000.bin")
            .expect("dir00/f000.bin must exist");
        let content = cat
            .read_file_decompressed(&mut dev, ino)
            .expect("read resolved file");
        assert_eq!(content.len(), 4096);
        assert!(content.iter().all(|&b| b == b'A'));

        assert!(cat
            .resolve_path(&mut dev, &["dir00", "does-not-exist"], hashed)
            .expect("resolve missing")
            .is_none());
        assert!(cat
            .resolve_path(&mut dev, &["nope", "x"], hashed)
            .expect("resolve missing parent")
            .is_none());
        assert_eq!(
            cat.resolve_path(&mut dev, &[], hashed)
                .expect("resolve root"),
            Some(crate::jkey::ROOT_DIR_INO_NUM)
        );
    }

    #[test]
    fn decompresses_real_macos_lzvn_files() {
        let img = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/apfs-large.img"
        );
        if !Path::new(img).exists() {
            eprintln!("skip: fixture missing - run `cargo run -p xtask -- gen-fixture --large`");
            return;
        }
        let mut dev = FileBlockDevice::open(img).expect("open fixture");
        let c = Container::open(&mut dev).expect("open container");
        let bsz = c.superblock.block_size;
        let vols = c.list_user_volumes(&mut dev).expect("list volumes");
        let vol = vols.first().expect("user volume").clone();
        let cat = Catalog::open(&mut dev, &vol, bsz).expect("open catalog");
        let hashed = vol.names_are_hashed();
        let root = cat
            .list_dir(&mut dev, ROOT_DIR_INO_NUM, hashed)
            .expect("root");
        let dir00 = match root.iter().find(|e| e.name == "dir00") {
            Some(d) => d.clone(),
            None => {
                eprintln!("skip: size-only fixture - regenerate gen-fixture --large");
                return;
            }
        };
        let kids = cat
            .list_dir(&mut dev, dir00.file_id, hashed)
            .expect("dir00");
        let find = |n: &str| kids.iter().find(|e| e.name == n).cloned();

        let mut checked = 0;
        for tag in ["s", "m"] {
            let bin = find(&format!("cmp_{tag}.bin"));
            let exp = find(&format!("cmp_{tag}.expected"));
            match (bin, exp) {
                (Some(b), Some(e)) => {
                    let got = cat
                        .read_file_decompressed(&mut dev, b.file_id)
                        .unwrap_or_else(|_| panic!("decompress real macOS LZVN sample {tag}"));
                    let want = cat.read_file(&mut dev, e.file_id).expect("expected");
                    assert_eq!(
                        got.len(),
                        want.len(),
                        "sample {tag}: decompressed length must equal macOS kernel output"
                    );
                    assert!(
                        got == want,
                        "sample {tag}: LZVN bytes must equal macOS kernel's own decompression"
                    );
                    checked += 1;
                }
                _ => eprintln!("note: M3d LZVN sample '{tag}' absent (M3D_FIXTURE_SKIP at gen)"),
            }
        }
        if checked == 0 {
            eprintln!(
                "skip: no M3d LZVN samples in fixture - re-run gen-fixture --large on a \
                 macOS host with readable /usr/bin compressed files"
            );
            return;
        }

        // Non-compressed regression: f000.bin reads via the fallback path.
        let f000 = find("f000.bin").expect("f000.bin");
        let plain = cat
            .read_file_decompressed(&mut dev, f000.file_id)
            .expect("plain via decompressed path");
        assert_eq!(plain.len(), 4096);
        assert!(plain.iter().all(|&b| b == b'A'));
    }

    #[test]
    fn stat_real_fixture_size_and_isdir() {
        let img = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/apfs-large.img"
        );
        if !Path::new(img).exists() {
            eprintln!("skip: fixture missing - run `cargo run -p xtask -- gen-fixture --large`");
            return;
        }
        let mut dev = FileBlockDevice::open(img).expect("open fixture");
        let c = Container::open(&mut dev).expect("open container");
        let bsz = c.superblock.block_size;
        let vol = c
            .list_user_volumes(&mut dev)
            .expect("vols")
            .first()
            .expect("vol")
            .clone();
        let cat = Catalog::open(&mut dev, &vol, bsz).expect("catalog");
        let hashed = vol.names_are_hashed();
        let dir00 = match cat
            .resolve_path(&mut dev, &["dir00"], hashed)
            .expect("resolve dir00")
        {
            Some(i) => i,
            None => {
                eprintln!("skip: size-only fixture - regenerate gen-fixture --large");
                return;
            }
        };
        let f000 = cat
            .resolve_path(&mut dev, &["dir00", "f000.bin"], hashed)
            .expect("resolve f000")
            .expect("f000 exists");
        let ds = cat.stat(&mut dev, dir00).expect("stat dir00");
        assert!(ds.is_dir, "dir00 must be a directory");
        let fs = cat.stat(&mut dev, f000).expect("stat f000");
        assert!(!fs.is_dir, "f000.bin is a regular file");
        assert_eq!(fs.size, 4096, "f000.bin logical size");
    }

    // --- read_stream error branches ---

    /// MemDev that returns BlockError for any access beyond its data.
    struct BoundedMemDev {
        blocks: std::collections::HashMap<u64, Vec<u8>>,
        block_size: usize,
    }
    impl BoundedMemDev {
        fn new(block_size: usize) -> Self {
            Self {
                blocks: std::collections::HashMap::new(),
                block_size,
            }
        }
        fn fill_block(&mut self, paddr: u64, byte: u8) {
            let data = vec![byte; self.block_size];
            self.blocks.insert(paddr * self.block_size as u64, data);
        }
    }
    impl BlockDevice for BoundedMemDev {
        fn size(&self) -> u64 {
            64 * self.block_size as u64
        }
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            if let Some(data) = self.blocks.get(&offset) {
                let n = buf.len().min(data.len());
                buf[..n].copy_from_slice(&data[..n]);
                if n < buf.len() {
                    buf[n..].fill(0);
                }
                Ok(())
            } else if offset + buf.len() as u64 <= self.size() {
                buf.fill(0);
                Ok(())
            } else {
                Err(BlockError::OutOfRange {
                    offset,
                    len: buf.len() as u64,
                    size: self.size(),
                })
            }
        }
    }

    /// Build a Catalog backed by a given leaf node (no omap traversal needed).
    fn catalog_from_leaf(leaf_raw: Vec<u8>) -> Catalog {
        let root = BtreeNode::parse(&leaf_raw).expect("parse root");
        Catalog {
            omap: dummy_omap(),
            root,
            xid: 1,
            block_size: 4096,
        }
    }

    #[test]
    fn read_stream_truncated_request_returns_only_size_bytes() {
        // request size < actual extent length: output must be clamped to size.
        const BSZ: u32 = 4096;
        const STREAM_OID: u64 = 77;
        let raw_root = build_extent_leaf(
            STREAM_OID,
            &[(0, BSZ as u64, 5)], // one extent at paddr=5
        );
        let mut dev = MemDev::new(BSZ as usize);
        dev.fill_block(5, 0xAB);
        let cat = catalog_from_leaf(raw_root);
        // Request only 512 bytes out of a 4096-byte extent.
        let data = cat
            .read_stream(&mut dev, STREAM_OID, 512)
            .expect("read_stream 512");
        assert_eq!(data.len(), 512, "output must be clamped to requested size");
        assert!(
            data.iter().all(|&b| b == 0xAB),
            "first 512 bytes must be from paddr=5"
        );
    }

    #[test]
    fn read_stream_size_zero_returns_empty() {
        // Requesting 0 bytes must return an empty vec without touching device.
        const STREAM_OID: u64 = 88;
        let raw_root = build_extent_leaf(STREAM_OID, &[(0, 4096, 5)]);
        let mut dev = MemDev::new(4096);
        dev.fill_block(5, 0xCC);
        let cat = catalog_from_leaf(raw_root);
        let data = cat
            .read_stream(&mut dev, STREAM_OID, 0)
            .expect("read_stream 0");
        assert!(data.is_empty(), "size=0 must yield empty vec");
    }

    #[test]
    fn read_stream_no_matching_oid_returns_zeros() {
        // The extent leaf has entries for OID 200; requesting OID 201 finds nothing.
        // Result must be a zero-filled vec of the requested size.
        const BSZ: usize = 4096;
        const STREAM_OID: u64 = 200;
        let raw_root = build_extent_leaf(STREAM_OID, &[(0, BSZ as u64, 7)]);
        let mut dev = MemDev::new(BSZ);
        dev.fill_block(7, 0xDD);
        let cat = catalog_from_leaf(raw_root);
        let data = cat
            .read_stream(&mut dev, 201, BSZ)
            .expect("read_stream mismatched oid");
        assert_eq!(data.len(), BSZ);
        assert!(data.iter().all(|&b| b == 0), "no extents → all zeros");
    }

    #[test]
    fn read_stream_multiple_extents_assembled_in_order() {
        // Two non-overlapping extents at different logical offsets; their data must
        // appear at the correct positions in the output.
        const BSZ: usize = 4096;
        const STREAM_OID: u64 = 55;
        let raw_root = build_extent_leaf(
            STREAM_OID,
            &[
                (BSZ as u64, BSZ as u64, 6), // second logical block at paddr=6
                (0, BSZ as u64, 5),          // first logical block at paddr=5
            ],
        );
        let mut dev = MemDev::new(BSZ);
        dev.fill_block(5, 0x11); // logical block 0 content
        dev.fill_block(6, 0x22); // logical block 1 content
        let cat = catalog_from_leaf(raw_root);
        let data = cat
            .read_stream(&mut dev, STREAM_OID, BSZ * 2)
            .expect("two extents");
        assert_eq!(data.len(), BSZ * 2);
        assert!(
            data[..BSZ].iter().all(|&b| b == 0x11),
            "first extent at offset 0"
        );
        assert!(
            data[BSZ..].iter().all(|&b| b == 0x22),
            "second extent at offset BSZ"
        );
    }

    // --- for_each_leaf depth guard ---

    #[test]
    fn for_each_leaf_empty_leaf_calls_closure_zero_times() {
        // A leaf node with nkeys=0 must invoke the closure zero times.
        const BSZ: usize = 4096;
        let mut b = vec![0u8; BSZ];
        b[24..28].copy_from_slice(&OBJECT_TYPE_BTREE.to_le_bytes());
        b[32..34].copy_from_slice(&BTNODE_LEAF.to_le_bytes());
        b[34..36].copy_from_slice(&0u16.to_le_bytes()); // level
        b[36..40].copy_from_slice(&0u32.to_le_bytes()); // nkeys = 0
        b[40..42].copy_from_slice(&0u16.to_le_bytes()); // toc_off
        b[42..44].copy_from_slice(&0u16.to_le_bytes()); // toc_len
        let ck = fletcher64(&b);
        b[0..8].copy_from_slice(&ck.to_le_bytes());
        let root = BtreeNode::parse(&b).expect("parse empty leaf");
        let cat = Catalog {
            omap: dummy_omap(),
            root,
            xid: 1,
            block_size: 4096,
        };
        let mut count = 0usize;
        let mut dev = MemDev::new(BSZ);
        cat.for_each_leaf(&mut dev, &cat.root.clone(), 0, &mut |_k, _v| {
            count += 1;
            Ok(())
        })
        .expect("for_each_leaf on empty node");
        assert_eq!(count, 0, "empty leaf must call closure zero times");
    }

    // ---------------------------------------------------------------------------
    // an audit pass - stat() pure unit tests
    // ---------------------------------------------------------------------------

    /// Build a minimal var-kv leaf containing a single APFS_TYPE_INODE entry for
    /// `inode_num`. The inode value is the raw 92-byte j_inode_val_t with no
    /// xfields (num_exts=0).
    ///
    /// Fields written (all LE):
    ///   [0..8]   parent_id
    ///   [8..16]  private_id
    ///   [16..24] create_time = 0
    ///   [24..32] mod_time    = 0
    ///   [32..40] change_time = 0
    ///   [40..48] access_time = 0
    ///   [48..56] internal_flags = 0
    ///   [56..60] nchildren_or_nlink = 0
    ///   [60..64] default_protection_class = 0
    ///   [64..68] write_generation_counter = 0
    ///   [68..72] bsd_flags
    ///   [72..76] uid
    ///   [76..80] gid
    ///   [80..82] mode
    ///   [82..84] pad1 = 0
    ///   [84..92] pad2 = 0
    ///   [92..94] xf_num_exts = 0
    ///   [94..96] xf_used_data = 0
    fn build_inode_leaf(inode_num: u64, mode: u16, bsd_flags: u32) -> Vec<u8> {
        let bsz = 4096usize;
        let data_base: usize = 56;

        // Key: j_key_t (8 bytes) only - no per-key extension for inode keys
        let key_size = 8usize;
        // Value: 96-byte j_inode_val_t (92 fixed + 4 bytes xf_blob header)
        let val_size = 96usize;
        let toc_len = 8usize; // one kvloc_t entry
        let val_area_end = bsz;

        let mut b = vec![0u8; bsz];
        b[24..28].copy_from_slice(&OBJECT_TYPE_BTREE.to_le_bytes());
        b[32..34].copy_from_slice(&BTNODE_LEAF.to_le_bytes());
        b[34..36].copy_from_slice(&0u16.to_le_bytes()); // level = 0
        b[36..40].copy_from_slice(&1u32.to_le_bytes()); // nkeys = 1
        b[40..42].copy_from_slice(&0u16.to_le_bytes()); // toc_off
        b[42..44].copy_from_slice(&(toc_len as u16).to_le_bytes()); // toc_len

        // j_key_t for TYPE_INODE
        let jkey_word = (inode_num & OBJ_ID_MASK) | ((APFS_TYPE_INODE as u64) << OBJ_TYPE_SHIFT);
        let key_area_start = data_base + toc_len;
        b[key_area_start..key_area_start + 8].copy_from_slice(&jkey_word.to_le_bytes());

        // j_inode_val_t (96 bytes = 92 fixed + 4-byte xf_blob_t header)
        let mut val = vec![0u8; val_size];
        val[0..8].copy_from_slice(&inode_num.to_le_bytes()); // parent_id
        val[8..16].copy_from_slice(&inode_num.to_le_bytes()); // private_id
        val[68..72].copy_from_slice(&bsd_flags.to_le_bytes());
        val[80..82].copy_from_slice(&mode.to_le_bytes());
        // xf_num_exts = 0 at offset 92
        val[92..94].copy_from_slice(&0u16.to_le_bytes());
        val[94..96].copy_from_slice(&0u16.to_le_bytes());

        let v_abs = val_area_end - val_size;
        let v_off = val_area_end - v_abs;
        let k_off = 0usize; // relative to key_area_start

        // kvloc_t at toc position
        let toc_entry = data_base;
        b[toc_entry..toc_entry + 2].copy_from_slice(&(k_off as u16).to_le_bytes());
        b[toc_entry + 2..toc_entry + 4].copy_from_slice(&(key_size as u16).to_le_bytes());
        b[toc_entry + 4..toc_entry + 6].copy_from_slice(&(v_off as u16).to_le_bytes());
        b[toc_entry + 6..toc_entry + 8].copy_from_slice(&(val_size as u16).to_le_bytes());

        b[v_abs..v_abs + val_size].copy_from_slice(&val);

        let ck = fletcher64(&b);
        b[0..8].copy_from_slice(&ck.to_le_bytes());
        b
    }

    #[test]
    fn stat_inode_not_found_returns_error() {
        // Leaf has inode 42; requesting inode 99 must return ContainerError.
        let raw = build_inode_leaf(42, 0o100644, 0);
        let cat = catalog_from_leaf(raw);
        let mut dev = MemDev::new(4096);
        assert!(cat.stat(&mut dev, 99).is_err(), "unknown inode must error");
    }

    #[test]
    fn stat_regular_file_inode_returns_correct_fields() {
        // S_IFREG | 0644 = 0o100644 = 0x81A4
        const MODE_REG: u16 = 0o100644;
        const BSD_FLAGS: u32 = 0x0000_8000; // UF_HIDDEN
        const INODE: u64 = 55;
        let raw = build_inode_leaf(INODE, MODE_REG, BSD_FLAGS);
        let cat = catalog_from_leaf(raw);
        let mut dev = MemDev::new(4096);
        let s = cat.stat(&mut dev, INODE).expect("stat regular file");
        assert!(!s.is_dir, "regular file must not be is_dir");
        assert_eq!(s.mode, MODE_REG, "mode round-trips");
        assert_eq!(s.bsd_flags, BSD_FLAGS, "bsd_flags round-trips");
        assert_eq!(s.size, 0, "no dstream → size=0");
    }

    #[test]
    fn stat_directory_inode_sets_is_dir() {
        // S_IFDIR | 0755 = 0o040755 = 0x41ED
        const MODE_DIR: u16 = 0o040755;
        const INODE: u64 = 77;
        let raw = build_inode_leaf(INODE, MODE_DIR, 0);
        let cat = catalog_from_leaf(raw);
        let mut dev = MemDev::new(4096);
        let s = cat.stat(&mut dev, INODE).expect("stat directory");
        assert!(s.is_dir, "directory inode must have is_dir=true");
        assert_eq!(s.mode, MODE_DIR);
    }

    // -----------------------------------------------------------------------
    // build_xattr_leaf: produce a var-kv leaf with one embedded xattr record.
    //
    // j_xattr_key_t layout:
    //   [0..8]  j_key_t: (obj_id & OBJ_ID_MASK) | (APFS_TYPE_XATTR << 60)
    //   [8..10] name_len (u16, includes trailing NUL)
    //   [10..]  name bytes + NUL
    //
    // j_xattr_val_t layout:
    //   [0..2]  flags (u16)
    //   [2..4]  xdata_len (u16)
    //   [4..]   xdata bytes
    // -----------------------------------------------------------------------
    fn build_xattr_leaf(inode_num: u64, attr_name: &str, attr_data: &[u8]) -> Vec<u8> {
        use crate::jkey::{APFS_TYPE_XATTR, OBJ_ID_MASK, OBJ_TYPE_SHIFT};
        use crate::xattr::XATTR_DATA_EMBEDDED;

        let bsz = 4096usize;
        let data_base: usize = 56;

        // Build key bytes
        let name_bytes: Vec<u8> = attr_name.bytes().chain(std::iter::once(0u8)).collect();
        let name_len = name_bytes.len() as u16;
        let jkey_word = (inode_num & OBJ_ID_MASK) | ((APFS_TYPE_XATTR as u64) << OBJ_TYPE_SHIFT);
        let mut key: Vec<u8> = Vec::new();
        key.extend_from_slice(&jkey_word.to_le_bytes());
        key.extend_from_slice(&name_len.to_le_bytes());
        key.extend_from_slice(&name_bytes);
        let key_size = key.len();

        // Build val bytes: flags=EMBEDDED, xdata_len, xdata
        let mut val: Vec<u8> = Vec::new();
        val.extend_from_slice(&XATTR_DATA_EMBEDDED.to_le_bytes());
        val.extend_from_slice(&(attr_data.len() as u16).to_le_bytes());
        val.extend_from_slice(attr_data);
        let val_size = val.len();

        let toc_len = 8usize; // one kvloc_t (8 bytes)
        let key_area_start = data_base + toc_len;
        let val_area_end = bsz;

        let mut b = vec![0u8; bsz];
        b[24..28].copy_from_slice(&OBJECT_TYPE_BTREE.to_le_bytes());
        b[32..34].copy_from_slice(&BTNODE_LEAF.to_le_bytes()); // var-kv leaf
        b[36..40].copy_from_slice(&1u32.to_le_bytes()); // nkeys=1
        b[40..42].copy_from_slice(&0u16.to_le_bytes()); // toc_off=0
        b[42..44].copy_from_slice(&(toc_len as u16).to_le_bytes());

        // kvloc_t: k_off=0, k_len, v_off, v_len
        let v_off = val_size as u16;
        b[data_base..data_base + 2].copy_from_slice(&0u16.to_le_bytes());
        b[data_base + 2..data_base + 4].copy_from_slice(&(key_size as u16).to_le_bytes());
        b[data_base + 4..data_base + 6].copy_from_slice(&v_off.to_le_bytes());
        b[data_base + 6..data_base + 8].copy_from_slice(&(val_size as u16).to_le_bytes());

        // Write key
        b[key_area_start..key_area_start + key_size].copy_from_slice(&key);

        // Write val (counting back from val_area_end)
        let v_abs = val_area_end - val_size;
        b[v_abs..v_abs + val_size].copy_from_slice(&val);

        let ck = fletcher64(&b);
        b[0..8].copy_from_slice(&ck.to_le_bytes());
        b
    }

    #[test]
    fn list_xattrs_synthetic_returns_entry() {
        let inode: u64 = 55;
        let raw = build_xattr_leaf(inode, "user.test", b"value");
        let cat = catalog_from_leaf(raw);
        let mut dev = MemDev::new(4096);
        let entries = cat.list_xattrs(&mut dev, inode).expect("list_xattrs");
        assert_eq!(entries.len(), 1, "must find one xattr");
        assert_eq!(entries[0].name, "user.test");
    }

    #[test]
    fn list_xattrs_wrong_inode_returns_empty() {
        let raw = build_xattr_leaf(55, "user.test", b"value");
        let cat = catalog_from_leaf(raw);
        let mut dev = MemDev::new(4096);
        let entries = cat
            .list_xattrs(&mut dev, 999)
            .expect("list_xattrs wrong inode");
        assert!(entries.is_empty(), "wrong inode must return no xattrs");
    }

    #[test]
    fn get_xattr_embedded_returns_data() {
        let inode: u64 = 66;
        let data = b"hello-world";
        let raw = build_xattr_leaf(inode, "user.embed", data);
        let cat = catalog_from_leaf(raw);
        let mut dev = MemDev::new(4096);
        let result = cat
            .get_xattr(&mut dev, inode, "user.embed")
            .expect("get_xattr ok")
            .expect("must be Some");
        assert_eq!(result, data, "embedded xattr must return exact bytes");
    }

    #[test]
    fn get_xattr_absent_returns_none() {
        let raw = build_xattr_leaf(66, "user.embed", b"x");
        let cat = catalog_from_leaf(raw);
        let mut dev = MemDev::new(4096);
        let result = cat.get_xattr(&mut dev, 66, "user.nope").expect("ok");
        assert!(result.is_none(), "absent xattr must return None");
    }

    #[test]
    fn for_each_leaf_depth_guard_terminates() {
        // Construct a non-leaf node whose child omap resolution will fail (omap
        // has no entries). The depth guard at 32 must prevent infinite descent
        // and return Ok(()) rather than panicking or looping forever.
        use crate::jkey::{OBJ_ID_MASK, OBJ_TYPE_SHIFT};
        // APFS_TYPE_DIR_REC = 9
        const APFS_TYPE_DIR_REC_VAL: u64 = 9;

        let bsz = 4096usize;
        let data_base = 56usize;

        // Build a non-leaf var-kv node with one entry (a fake child oid).
        // Non-leaf values in catalog: 8-byte virtual child oid.
        let child_oid: u64 = 0xDEAD; // won't be in omap → resolve returns None → oor()

        let jkey_word = (2u64 & OBJ_ID_MASK) | (APFS_TYPE_DIR_REC_VAL << OBJ_TYPE_SHIFT);
        let mut key = Vec::new();
        key.extend_from_slice(&jkey_word.to_le_bytes()); // 8 bytes j_key_t
        key.extend_from_slice(&0u16.to_le_bytes()); // 2-byte name_len=0
                                                    // name: empty (just NUL)
        key.push(0u8);
        let key_size = key.len();

        let mut val = Vec::new();
        val.extend_from_slice(&child_oid.to_le_bytes()); // 8-byte child oid
        let val_size = val.len();

        let toc_len = 8usize;
        let key_area_start = data_base + toc_len;
        let val_area_end = bsz;

        let mut b = vec![0u8; bsz];
        b[24..28].copy_from_slice(&OBJECT_TYPE_BTREE.to_le_bytes());
        // Non-leaf: level=1, NOT BTNODE_LEAF
        b[34..36].copy_from_slice(&1u16.to_le_bytes()); // level=1
        b[36..40].copy_from_slice(&1u32.to_le_bytes()); // nkeys=1
        b[40..42].copy_from_slice(&0u16.to_le_bytes()); // toc_off=0
        b[42..44].copy_from_slice(&(toc_len as u16).to_le_bytes());

        b[data_base..data_base + 2].copy_from_slice(&0u16.to_le_bytes());
        b[data_base + 2..data_base + 4].copy_from_slice(&(key_size as u16).to_le_bytes());
        b[data_base + 4..data_base + 6].copy_from_slice(&(val_size as u16).to_le_bytes());
        b[data_base + 6..data_base + 8].copy_from_slice(&(val_size as u16).to_le_bytes());

        b[key_area_start..key_area_start + key_size].copy_from_slice(&key);
        let v_abs = val_area_end - val_size;
        b[v_abs..v_abs + val_size].copy_from_slice(&val);

        let ck = fletcher64(&b);
        b[0..8].copy_from_slice(&ck.to_le_bytes());

        // catalog_from_leaf builds Catalog with dummy_omap (empty, resolve → None)
        let cat = catalog_from_leaf(b);
        let mut dev = MemDev::new(4096);
        // list_dir calls for_each_leaf; non-leaf child resolution fails (omap returns None)
        // → oor() error propagated. The key thing: no panic, no infinite loop.
        let _ = cat.list_dir(&mut dev, 2, false); // ok if Err (oor), not ok if panic
    }

    #[test]
    fn catalog_open_rejects_bad_root_object_type() {
        // Build a real fixture-style open via MemDev with:
        //   - valid omap block at addr 1
        //   - omap tree root (empty FIXED_KV leaf) at addr 2
        //   - volume omap resolves root_tree_oid → paddr 3
        //   - block 3 has wrong object type (not OBJECT_TYPE_BTREE)
        use crate::btree::{BTNODE_FIXED_KV_SIZE, BTNODE_LEAF, BTNODE_ROOT};
        use crate::obj::{ObjPhys, OBJECT_TYPE_OMAP};
        use crate::volume::{VolumeSuperblock, APFS_MAGIC};

        // Build a VolumeSuperblock with omap_oid=1, root_tree_oid=42, xid=1.
        // We craft the raw block manually to control exact bytes.
        use crate::obj::OBJECT_TYPE_FS;
        let mut vsb_raw = [0u8; 4096];
        vsb_raw[24..28].copy_from_slice(&OBJECT_TYPE_FS.to_le_bytes()); // o_type
        vsb_raw[16..24].copy_from_slice(&1u64.to_le_bytes()); // xid=1
        vsb_raw[32..36].copy_from_slice(&APFS_MAGIC.to_le_bytes());
        vsb_raw[128..136].copy_from_slice(&1u64.to_le_bytes()); // omap_oid=1 (physical)
        vsb_raw[136..144].copy_from_slice(&42u64.to_le_bytes()); // root_tree_oid=42 (virtual)
                                                                 // uuid at 240 (16 bytes), name at 704 - leave as zeros, role at 964
        let ck = fletcher64(&vsb_raw);
        vsb_raw[0..8].copy_from_slice(&ck.to_le_bytes());
        let vol = VolumeSuperblock::parse(&vsb_raw).expect("parse vsb");

        // Build omap at block 1 (tree_oid=2)
        let mut omap_raw = vec![0u8; 4096];
        omap_raw[8..16].copy_from_slice(&1u64.to_le_bytes());
        omap_raw[24..28].copy_from_slice(&OBJECT_TYPE_OMAP.to_le_bytes());
        omap_raw[48..56].copy_from_slice(&2u64.to_le_bytes()); // tree_oid=2
        let ck2 = fletcher64(&omap_raw);
        omap_raw[0..8].copy_from_slice(&ck2.to_le_bytes());

        // Build FIXED_KV ROOT LEAF at block 2 with one entry: key(oid=42,xid=1)→paddr=3
        let mut root_raw = vec![0u8; 4096];
        root_raw[8..16].copy_from_slice(&2u64.to_le_bytes());
        root_raw[16..24].copy_from_slice(&1u64.to_le_bytes());
        root_raw[24..28].copy_from_slice(&OBJECT_TYPE_BTREE.to_le_bytes());
        let flags = BTNODE_LEAF | BTNODE_FIXED_KV_SIZE | BTNODE_ROOT;
        root_raw[32..34].copy_from_slice(&flags.to_le_bytes());
        root_raw[36..40].copy_from_slice(&1u32.to_le_bytes()); // nkeys=1
        root_raw[40..42].copy_from_slice(&0u16.to_le_bytes()); // toc_off=0
        root_raw[42..44].copy_from_slice(&4u16.to_le_bytes()); // toc_len=4 (1 entry)
                                                               // DATA_BASE=56: kvoff_t { k_off=0, v_off=16 }
        root_raw[56..58].copy_from_slice(&0u16.to_le_bytes());
        root_raw[58..60].copy_from_slice(&16u16.to_le_bytes());
        // key at 60: omap_key {oid=42, xid=1}
        root_raw[60..68].copy_from_slice(&42u64.to_le_bytes());
        root_raw[68..76].copy_from_slice(&1u64.to_le_bytes());
        // ROOT: val_area_end = 4096-40=4056; val at 4056-16=4040: omap_val {flags=0,size=4096,paddr=3}
        root_raw[4040..4044].copy_from_slice(&0u32.to_le_bytes()); // flags=0 (not deleted)
        root_raw[4044..4048].copy_from_slice(&4096u32.to_le_bytes());
        root_raw[4048..4056].copy_from_slice(&3i64.to_le_bytes()); // paddr=3
        let ck3 = fletcher64(&root_raw);
        root_raw[0..8].copy_from_slice(&ck3.to_le_bytes());

        // Block 3: wrong object type (OBJECT_TYPE_OMAP instead of BTREE)
        let mut bad_cat_root = vec![0u8; 4096];
        bad_cat_root[8..16].copy_from_slice(&3u64.to_le_bytes());
        bad_cat_root[24..28].copy_from_slice(&OBJECT_TYPE_OMAP.to_le_bytes());
        let ck4 = fletcher64(&bad_cat_root);
        bad_cat_root[0..8].copy_from_slice(&ck4.to_le_bytes());

        // Assemble MemDev (block-addressed)
        struct AddrMemDev {
            blocks: std::collections::HashMap<u64, Vec<u8>>,
        }
        impl crate::block_device::BlockDevice for AddrMemDev {
            fn size(&self) -> u64 {
                4096 * 64
            }
            fn read_at(
                &mut self,
                off: u64,
                buf: &mut [u8],
            ) -> Result<(), crate::block_device::BlockError> {
                let addr = off / 4096;
                match self.blocks.get(&addr) {
                    Some(d) => {
                        let l = buf.len().min(d.len());
                        buf[..l].copy_from_slice(&d[..l]);
                        Ok(())
                    }
                    None => {
                        buf.fill(0);
                        Ok(())
                    }
                }
            }
        }
        let mut dev = AddrMemDev {
            blocks: std::collections::HashMap::new(),
        };
        dev.blocks.insert(1, omap_raw);
        dev.blocks.insert(2, root_raw);
        dev.blocks.insert(3, bad_cat_root);

        let err = Catalog::open(&mut dev, &vol, 4096);
        assert!(
            err.is_err(),
            "wrong catalog root object type must be rejected"
        );
    }

    // -----------------------------------------------------------------------
    // list_xattrs: leaf with two xattr entries for the same inode.
    // -----------------------------------------------------------------------
    #[test]
    fn list_xattrs_multiple_entries_same_inode() {
        use crate::jkey::{APFS_TYPE_XATTR, OBJ_ID_MASK, OBJ_TYPE_SHIFT};
        use crate::xattr::XATTR_DATA_EMBEDDED;

        let inode: u64 = 77;
        let bsz = 4096usize;
        let data_base = 56usize;

        // Build two xattr entries in one leaf.
        let make_xattr_kv = |name: &[u8], payload: &[u8]| -> (Vec<u8>, Vec<u8>) {
            let jkey = (inode & OBJ_ID_MASK) | ((APFS_TYPE_XATTR as u64) << OBJ_TYPE_SHIFT);
            let mut k = Vec::new();
            k.extend_from_slice(&jkey.to_le_bytes());
            k.extend_from_slice(&((name.len() + 1) as u16).to_le_bytes()); // name_len incl NUL
            k.extend_from_slice(name);
            k.push(0u8); // NUL
            let mut v = Vec::new();
            v.extend_from_slice(&XATTR_DATA_EMBEDDED.to_le_bytes());
            v.extend_from_slice(&(payload.len() as u16).to_le_bytes());
            v.extend_from_slice(payload);
            (k, v)
        };

        let (k1, v1) = make_xattr_kv(b"user.alpha", b"aaa");
        let (k2, v2) = make_xattr_kv(b"user.beta", b"bb");

        // Two-entry var-kv leaf.
        let toc_len = 2 * 8; // 2 × kvloc_t (8 bytes each)
        let key_area_start = data_base + toc_len;
        let val_area_end = bsz;

        let k1_off = 0usize;
        let k2_off = k1.len();
        let v1_abs = val_area_end - v1.len();
        let v2_abs = v1_abs - v2.len();

        let mut b = vec![0u8; bsz];
        b[24..28].copy_from_slice(&OBJECT_TYPE_BTREE.to_le_bytes());
        b[32..34].copy_from_slice(&BTNODE_LEAF.to_le_bytes());
        b[36..40].copy_from_slice(&2u32.to_le_bytes()); // nkeys=2
        b[40..42].copy_from_slice(&0u16.to_le_bytes());
        b[42..44].copy_from_slice(&(toc_len as u16).to_le_bytes());

        // toc entry 0
        b[data_base..data_base + 2].copy_from_slice(&(k1_off as u16).to_le_bytes());
        b[data_base + 2..data_base + 4].copy_from_slice(&(k1.len() as u16).to_le_bytes());
        b[data_base + 4..data_base + 6]
            .copy_from_slice(&((val_area_end - v1_abs) as u16).to_le_bytes());
        b[data_base + 6..data_base + 8].copy_from_slice(&(v1.len() as u16).to_le_bytes());
        // toc entry 1
        b[data_base + 8..data_base + 10].copy_from_slice(&(k2_off as u16).to_le_bytes());
        b[data_base + 10..data_base + 12].copy_from_slice(&(k2.len() as u16).to_le_bytes());
        b[data_base + 12..data_base + 14]
            .copy_from_slice(&((val_area_end - v2_abs) as u16).to_le_bytes());
        b[data_base + 14..data_base + 16].copy_from_slice(&(v2.len() as u16).to_le_bytes());

        b[key_area_start..key_area_start + k1.len()].copy_from_slice(&k1);
        b[key_area_start + k1.len()..key_area_start + k1.len() + k2.len()].copy_from_slice(&k2);
        b[v1_abs..v1_abs + v1.len()].copy_from_slice(&v1);
        b[v2_abs..v2_abs + v2.len()].copy_from_slice(&v2);

        let ck = fletcher64(&b);
        b[0..8].copy_from_slice(&ck.to_le_bytes());

        let cat = catalog_from_leaf(b);
        let mut dev = MemDev::new(4096);
        let entries = cat.list_xattrs(&mut dev, inode).expect("list_xattrs");
        assert_eq!(entries.len(), 2, "must find both xattr entries");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"user.alpha"), "must contain user.alpha");
        assert!(names.contains(&"user.beta"), "must contain user.beta");
    }

    // -----------------------------------------------------------------------
    // get_xattr: name present but different inode → None (inode filter).
    // -----------------------------------------------------------------------
    #[test]
    fn get_xattr_different_inode_returns_none() {
        // Leaf has xattr for inode 55; query for inode 56 must return None.
        let raw = build_xattr_leaf(55, "user.x", b"data");
        let cat = catalog_from_leaf(raw);
        let mut dev = MemDev::new(4096);
        let result = cat.get_xattr(&mut dev, 56, "user.x").expect("no error");
        assert!(
            result.is_none(),
            "xattr keyed to different inode must return None"
        );
    }

    // -----------------------------------------------------------------------
    // get_xattr: correct inode, wrong name → None.
    // -----------------------------------------------------------------------
    #[test]
    fn get_xattr_wrong_name_same_inode_returns_none() {
        let raw = build_xattr_leaf(42, "user.present", b"data");
        let cat = catalog_from_leaf(raw);
        let mut dev = MemDev::new(4096);
        let result = cat
            .get_xattr(&mut dev, 42, "user.absent")
            .expect("no error");
        assert!(
            result.is_none(),
            "wrong name must return None even if inode matches"
        );
    }
}
