all: lint-fix lint test

lint:
	golangci-lint run

lint-fix:
	golangci-lint run --fix

fmt:
	gofumpt -w .

test:
	go test -v ./...

install:
	go install



