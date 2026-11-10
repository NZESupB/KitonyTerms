package main

import (
	"encoding/binary"
	"errors"
)

// 只接受生成器使用的三层数字ID目录，先限制节点/累计资源，避免解析重复引用放大。
func validateResourceDirectory(data []byte, base uint32) error {
	invalid := errors.New("PE资源目录越界、重复或超限")
	if len(data) == 0 || len(data) > maxResourceBytes {
		return invalid
	}
	read := func(offset uint64, size uint64) ([]byte, bool) {
		if offset > uint64(len(data)) || size > uint64(len(data))-offset {
			return nil, false
		}
		return data[offset : offset+size], true
	}
	visited := map[uint32]bool{}
	var nodes, total uint64
	var visit func(uint32, int) error
	visit = func(offset uint32, depth int) error {
		if depth > 2 || visited[offset] {
			return invalid
		}
		visited[offset] = true
		header, ok := read(uint64(offset), 16)
		if !ok || binary.LittleEndian.Uint16(header[12:]) != 0 {
			return invalid
		}
		count := uint64(binary.LittleEndian.Uint16(header[14:]))
		nodes += count
		if count == 0 || nodes > 512 {
			return invalid
		}
		entries, ok := read(uint64(offset)+16, count*8)
		if !ok {
			return invalid
		}
		ids := map[uint32]bool{}
		for i := uint64(0); i < count; i++ {
			entry := entries[i*8 : i*8+8]
			id, child := binary.LittleEndian.Uint32(entry), binary.LittleEndian.Uint32(entry[4:])
			if id == 0 && depth < 2 || id > 65535 || ids[id] {
				return invalid
			}
			ids[id] = true
			if depth < 2 {
				if child&0x80000000 == 0 {
					return invalid
				}
				if err := visit(child&0x7fffffff, depth+1); err != nil {
					return err
				}
				continue
			}
			if child&0x80000000 != 0 {
				return invalid
			}
			leaf, ok := read(uint64(child), 16)
			if !ok {
				return invalid
			}
			rva, size := binary.LittleEndian.Uint32(leaf), uint64(binary.LittleEndian.Uint32(leaf[4:]))
			total += size
			if rva < base || size == 0 || total > maxResourceBytes {
				return invalid
			}
			if _, ok = read(uint64(rva)-uint64(base), size); !ok {
				return invalid
			}
		}
		return nil
	}
	return visit(0, 0)
}
