#!/bin/bash

agent () {
	port=$1
	agent_id=$2
	HALO_TEST_DIRECTORY=tests/test_output/$cluster_type cargo run --bin halo_remote -- \
		--network 127.0.0.0/24 \
		--port $port \
		--test-id $agent_id \
		--ocf-root tests/ocf_resources
}

manager () {
	config=tests/configs/$cluster_type.yaml
	cargo run --bin halo_manager -- \
		--config $config \
		--socket halo.socket \
		--verbose \
		--sleep-time 2000 \
		--fence-on-connection-close \
		--statefile halo_$cluster_type.state \
		--manage-resources
}

lustre () {
	case $role in
		agent1)		agent 8005 fence_mds00;;
		agent2)		agent 8006 fence_mds01;;
		manager)	manager;;
	esac
}

nfs () {
	case $role in
		agent1)		agent 8005 fence_nfs00;;
		agent2)		agent 8006 fence_nfs01;;
		manager)	manager;;
	esac
}

usage () {
	echo usage: tests.sh [lustre|nfs] [agent1|agent2|manager]
}

cluster_type=$1
role=$2

mkdir -p tests/test_output/$cluster_type

case $cluster_type in
	lustre)		lustre;;
	nfs)		nfs;;
	*)		usage;;
esac
