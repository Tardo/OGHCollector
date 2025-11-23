#!/bin/bash
# Copyright 2025 Alexandre D. Díaz
ogh_cmd="docker compose run --rm -u appuser -l traefik.enable=false -T app oghcollector"
versions_openerp=("6.1" "7.0" "8.0" "9.0")
versions_odoo=("10.0" "11.0" "12.0" "13.0" "14.0" "15.0" "16.0" "17.0" "18.0" "19.0")
versions_all=("${versions_openerp[@]}" "${versions_odoo[@]}")

for oversion in "${versions_openerp[@]}"; do
    $ogh_cmd odoo/odoo:/addons,/openerp/addons $oversion
done

for oversion in "${versions_odoo[@]}"; do
    $ogh_cmd odoo/odoo:/addons,/odoo/addons $oversion
done

for oversion in "${versions_all[@]}"; do
    $ogh_cmd OCA $oversion
done
