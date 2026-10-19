module example.com/demo

go 1.22

toolchain go1.22.4

require github.com/BurntSushi/toml v1.3.2

require (
	github.com/gin-gonic/gin v1.9.0
	golang.org/x/net v0.0.0-20220722155237-a158d28d115b // indirect
	golang.org/x/text v0.3.7 // indirect
	gopkg.in/yaml.v3 v3.0.1
)

replace (
	example.com/old v1.0.0 => example.com/new v1.1.0
)

exclude github.com/gin-gonic/gin v1.8.0

retract v0.9.0 // published by mistake
