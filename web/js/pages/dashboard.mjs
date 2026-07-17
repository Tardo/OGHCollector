// Copyright 2025 Alexandre D. Díaz
import '@app/components/module-counter';
import '@app/components/module-search';
import '@scss/pages/dashboard.scss';
import {bindSearchModal} from '@app/utils/search-modal';

bindSearchModal('module_search', 'mirlo-module-search');
