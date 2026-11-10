package main

import (
	"bytes"
	"encoding/binary"
	"errors"
)

// 标准库也会按符号/重定位数量读表；先限定它实际会读取的字节和累计数量。
func validateMachOBounds(data []byte) error {
	invalid := func() error { return errors.New("Mach-O加载命令或符号表范围无效/超限") }
	if len(data) < 32 || binary.LittleEndian.Uint32(data[:4]) != 0xfeedfacf {
		return invalid()
	}
	order := binary.LittleEndian
	commands, size := uint64(order.Uint32(data[16:20])), uint64(order.Uint32(data[20:24]))
	if commands == 0 || commands > 256 || size > 1<<20 || size < commands*8 || size > uint64(len(data)-32) {
		return invalid()
	}
	within := func(offset, count uint64) bool {
		return offset <= uint64(len(data)) && count <= uint64(len(data))-offset
	}
	offset, end := uint64(32), uint64(32)+size
	var symbols, dynamic, relocations uint64
	for i := uint64(0); i < commands; i++ {
		if end-offset < 8 {
			return invalid()
		}
		command, length := order.Uint32(data[offset:offset+4]), uint64(order.Uint32(data[offset+4:offset+8]))
		if length < 8 || length%8 != 0 || length > end-offset {
			return invalid()
		}
		raw := data[offset : offset+length]
		switch command {
		case 0x1: // 64位候选不接受32位段声明。
			return invalid()
		case 0x19: // LC_SEGMENT_64
			if length < 72 {
				return invalid()
			}
			sections := uint64(order.Uint32(raw[64:68]))
			if sections > 256 || 72+sections*80 != length || !within(order.Uint64(raw[40:48]), order.Uint64(raw[48:56])) {
				return invalid()
			}
			for section := uint64(0); section < sections; section++ {
				entry := raw[72+section*80 : 72+(section+1)*80]
				count := uint64(order.Uint32(entry[60:64]))
				relocations += count
				if relocations > 65536 || !within(uint64(order.Uint32(entry[56:60])), count*8) {
					return invalid()
				}
			}
		case 0x2: // LC_SYMTAB
			symbols++
			if symbols > 1 || length != 24 {
				return invalid()
			}
			symoff, count := uint64(order.Uint32(raw[8:12])), uint64(order.Uint32(raw[12:16]))
			stroff, strsize := uint64(order.Uint32(raw[16:20])), uint64(order.Uint32(raw[20:24]))
			if count > 65536 || strsize > 16<<20 || !within(symoff, count*16) || !within(stroff, strsize) {
				return invalid()
			}
			names := uint64(0)
			for index := uint64(0); index < count; index++ {
				name := uint64(order.Uint32(data[symoff+index*16 : symoff+index*16+4]))
				if name >= strsize {
					return invalid()
				}
				available := min(strsize-name, 1025)
				length := bytes.IndexByte(data[stroff+name:stroff+name+available], 0)
				if length < 0 {
					return invalid()
				}
				names += uint64(length)
				if names > 8<<20 {
					return invalid()
				}
			}
		case 0xb: // LC_DYSYMTAB
			dynamic++
			if dynamic > 1 || length != 80 {
				return invalid()
			}
			count := uint64(order.Uint32(raw[60:64]))
			if count > 65536 || !within(uint64(order.Uint32(raw[56:60])), count*4) {
				return invalid()
			}
		}
		offset += length
	}
	if offset != end {
		return invalid()
	}
	return nil
}
