# model.bin

The export of a 12-layer model, layer0 to layer11. Each layer is three little-endian
float32 values, 12 bytes: weight, bias, scale. There is no header and no index: the
exporter wrote the layers one after another, in the order of their names sorted as text.
